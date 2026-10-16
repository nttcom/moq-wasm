use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use gstreamer::{self as gst, prelude::*};

use super::CompositeSettings;

type Span = (gst::ClockTime, gst::ClockTime);

/// Start and end of the frames an input has fed but the output has not yet
/// passed, in running time, sorted by start. The feeder records them with the
/// durations it gave the frames: decoders replace a buffer duration with one
/// derived from the stream's frame rate, which would drop a sparse source
/// (a screen share) from the grid between its frames.
#[derive(Clone, Default)]
pub(super) struct FrameSpans(Arc<Mutex<VecDeque<Span>>>);

impl FrameSpans {
    /// Frames come in decode order, so B-frames insert a few places from the
    /// back.
    pub(super) fn record(&self, start: gst::ClockTime, end: gst::ClockTime) {
        let mut spans = self.0.lock().expect("frame spans lock");
        let index = spans
            .iter()
            .rposition(|&(span_start, _)| span_start <= start)
            .map_or(0, |index| index + 1);
        spans.insert(index, (start, end));
    }

    /// Whether a frame overlaps the output frame `start..end`; frames that
    /// ended before it are forgotten.
    fn covers(&self, start: gst::ClockTime, end: gst::ClockTime) -> bool {
        let mut spans = self.0.lock().expect("frame spans lock");
        while spans
            .front()
            .is_some_and(|&(_, span_end)| span_end <= start)
        {
            spans.pop_front();
        }
        spans
            .front()
            .is_some_and(|&(span_start, _)| span_start < end)
    }
}

struct Tile {
    pad: gst::Pad,
    frames: FrameSpans,
}

/// Before each output frame, lays out the inputs whose video covers it as a
/// grid and hides the rest. A non-live compositor keeps drawing a pad's last
/// frame until the next one arrives, however long the frame lasted, so a
/// camera that went off would otherwise freeze in its tile.
pub(super) fn install(
    compositor: &gst::Element,
    frames: Vec<FrameSpans>,
    settings: CompositeSettings,
) -> Result<()> {
    let tiles = frames
        .into_iter()
        .enumerate()
        .map(|(index, frames)| {
            let pad = compositor
                .static_pad(&format!("sink_{index}"))
                .with_context(|| format!("compositor has no sink_{index}"))?;
            Ok(Tile { pad, frames })
        })
        .collect::<Result<Vec<_>>>()?;
    let shown = Mutex::new(Vec::new());
    compositor.set_property("emit-signals", true);
    compositor.connect("samples-selected", false, move |values| {
        let start = values.get(2).and_then(|value| value.get::<u64>().ok());
        let duration = values.get(4).and_then(|value| value.get::<u64>().ok());
        let (Some(start), Some(duration)) = (start, duration) else {
            return None;
        };
        let (start, end) = (
            gst::ClockTime::from_nseconds(start),
            gst::ClockTime::from_nseconds(start.saturating_add(duration)),
        );
        let visible: Vec<bool> = tiles
            .iter()
            .map(|tile| tile.frames.covers(start, end))
            .collect();
        let mut shown = shown.lock().expect("shown tiles lock");
        if *shown != visible {
            arrange(&tiles, &visible, &settings);
            *shown = visible;
        }
        None
    });
    Ok(())
}

fn arrange(tiles: &[Tile], visible: &[bool], settings: &CompositeSettings) {
    let grid = Grid::new(visible.iter().filter(|shown| **shown).count(), settings);
    let mut slot = 0;
    for (tile, shown) in tiles.iter().zip(visible) {
        if !shown {
            tile.pad.set_property("alpha", 0.0f64);
            continue;
        }
        let (xpos, ypos) = grid.position(slot);
        slot += 1;
        tile.pad.set_property("xpos", xpos as i32);
        tile.pad.set_property("ypos", ypos as i32);
        tile.pad.set_property("width", grid.tile_width as i32);
        tile.pad.set_property("height", grid.tile_height as i32);
        tile.pad.set_property("alpha", 1.0f64);
    }
}

/// Tile sizes are even so tile edges fall on 4:2:0 chroma sample boundaries.
struct Grid {
    columns: u32,
    tile_width: u32,
    tile_height: u32,
    left: u32,
    top: u32,
}

impl Grid {
    fn new(tiles: usize, settings: &CompositeSettings) -> Self {
        let tiles = (tiles as u32).max(1);
        let columns = (1..=tiles)
            .find(|columns| columns * columns >= tiles)
            .unwrap_or(1);
        let rows = tiles.div_ceil(columns);
        let tile_width = (settings.width / columns) & !1;
        let tile_height = (settings.height / rows) & !1;
        Self {
            columns,
            tile_width,
            tile_height,
            left: (settings.width - columns * tile_width) / 2,
            top: (settings.height - rows * tile_height) / 2,
        }
    }

    fn position(&self, slot: usize) -> (u32, u32) {
        let slot = slot as u32;
        (
            self.left + (slot % self.columns) * self.tile_width,
            self.top + (slot / self.columns) * self.tile_height,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> gst::ClockTime {
        gst::ClockTime::from_mseconds(millis)
    }

    fn span(start_ms: u64, end_ms: u64) -> Span {
        (ms(start_ms), ms(end_ms))
    }

    #[test]
    fn lays_tiles_out_in_a_square_grid() {
        // Arrange
        let settings = CompositeSettings::default();

        // Act
        let grid = Grid::new(3, &settings);

        // Assert: a 2x2 grid, the fourth cell stays black
        assert_eq!((grid.tile_width, grid.tile_height), (640, 360));
        assert_eq!(
            (0..3).map(|slot| grid.position(slot)).collect::<Vec<_>>(),
            [(0, 0), (640, 0), (0, 360)]
        );
    }

    #[test]
    fn a_single_tile_fills_the_output() {
        // Arrange
        let settings = CompositeSettings::default();

        // Act
        let grid = Grid::new(1, &settings);

        // Assert
        assert_eq!((grid.tile_width, grid.tile_height), (1280, 720));
        assert_eq!(grid.position(0), (0, 0));
    }

    #[test]
    fn a_gap_between_frames_hides_the_tile() {
        // Arrange
        let frames = FrameSpans::default();
        frames.record(ms(0), ms(100));
        frames.record(ms(1_500), ms(1_600));
        let (start, end) = span(500, 566);

        // Act
        let shown = frames.covers(start, end);

        // Assert: the ended frame is forgotten, the future one is kept
        assert!(!shown);
        assert_eq!(*frames.0.lock().unwrap(), [span(1_500, 1_600)]);
    }

    #[test]
    fn a_frame_overlapping_the_output_shows_the_tile() {
        // Arrange
        let frames = FrameSpans::default();
        frames.record(ms(0), ms(100));
        frames.record(ms(100), ms(200));

        // Act / Assert
        let (start, end) = span(133, 200);
        assert!(frames.covers(start, end));
    }

    #[test]
    fn keeps_b_frames_in_presentation_order() {
        // Arrange: decode order I0 P3 B1 B2
        let frames = FrameSpans::default();

        // Act
        for start in [0, 300, 100, 200] {
            frames.record(ms(start), ms(start + 100));
        }

        // Assert
        assert_eq!(
            *frames.0.lock().unwrap(),
            [span(0, 100), span(100, 200), span(200, 300), span(300, 400)]
        );
    }
}
