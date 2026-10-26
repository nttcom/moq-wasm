use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use gstreamer::{self as gst, prelude::*};

use super::{CompositeSettings, speaker::Speakers};

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

/// Strip tiles are 1/`STRIP_FRACTION` of the output width.
const STRIP_FRACTION: u32 = 5;

/// The video an input feeds into compositor pad `sink_<index>`, where
/// `index` is its place in the list `install` takes.
pub(super) struct Picture {
    pub(super) frames: FrameSpans,
    pub(super) source: Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Source {
    /// The input the picture belongs to.
    pub(super) owner: usize,
    pub(super) content: Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Content {
    Camera,
    ScreenShare,
}

struct Tile {
    pad: gst::Pad,
    frames: FrameSpans,
}

/// Before each output frame, puts a screen share or else the active
/// speaker's camera in focus, the other pictures that cover the frame in a
/// strip down the right edge, and hides the rest: a non-live compositor keeps
/// drawing a pad's last frame until the next one arrives, so a camera that
/// went off would freeze in its tile.
pub(super) fn install(
    compositor: &gst::Element,
    pictures: Vec<Picture>,
    speakers: Speakers,
    settings: CompositeSettings,
) -> Result<()> {
    let sources: Vec<Source> = pictures.iter().map(|picture| picture.source).collect();
    let tiles = pictures
        .into_iter()
        .enumerate()
        .map(|(index, picture)| {
            let pad = compositor
                .static_pad(&format!("sink_{index}"))
                .with_context(|| format!("compositor has no sink_{index}"))?;
            Ok(Tile {
                pad,
                frames: picture.frames,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let shown: Mutex<Option<Stage>> = Mutex::new(None);
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
        let mut shown = shown.lock().expect("shown stage lock");
        let previous_focus = shown.as_ref().map(|stage| stage.focus);
        let stage = Stage::choose(&sources, &visible, speakers.at(start), previous_focus);
        if *shown != stage {
            arrange(&tiles, stage.as_ref(), &settings);
            *shown = stage;
        }
        None
    });
    Ok(())
}

fn arrange(tiles: &[Tile], stage: Option<&Stage>, settings: &CompositeSettings) {
    let rects = stage.map(|stage| stage.rects(settings)).unwrap_or_default();
    for (index, tile) in tiles.iter().enumerate() {
        let Some((_, rect)) = rects.iter().find(|(shown, _)| *shown == index) else {
            tile.pad.set_property("alpha", 0.0f64);
            continue;
        };
        tile.pad.set_property("xpos", rect.x as i32);
        tile.pad.set_property("ypos", rect.y as i32);
        tile.pad.set_property("width", rect.width as i32);
        tile.pad.set_property("height", rect.height as i32);
        tile.pad.set_property("alpha", 1.0f64);
    }
}

/// The tile in focus and the visible others in the strip, top first, by tile
/// index.
#[derive(Debug, PartialEq)]
struct Stage {
    focus: usize,
    strip: Vec<usize>,
}

impl Stage {
    /// Focus goes to a visible screen share, keeping the one already in focus
    /// over a later one; else the speaker's visible camera; else the previous
    /// focus if visible; else the first visible tile. The speaker's camera
    /// heads the strip. `None` while no tile is visible.
    fn choose(
        sources: &[Source],
        visible: &[bool],
        speaker: Option<usize>,
        previous_focus: Option<usize>,
    ) -> Option<Self> {
        let is_visible = |tile: &usize| visible[*tile];
        let is_share = |tile: &usize| sources[*tile].content == Content::ScreenShare;
        let speaker_camera = speaker
            .and_then(|speaker| {
                let camera = Source {
                    owner: speaker,
                    content: Content::Camera,
                };
                sources.iter().position(|source| *source == camera)
            })
            .filter(is_visible);
        let previous_focus = previous_focus.filter(is_visible);
        let tiles = 0..sources.len();
        let focus = match tiles
            .clone()
            .find(|tile| is_visible(tile) && is_share(tile))
        {
            Some(first_share) => previous_focus.filter(is_share).unwrap_or(first_share),
            None => speaker_camera
                .or(previous_focus)
                .or_else(|| tiles.clone().find(is_visible))?,
        };
        let strip = speaker_camera
            .into_iter()
            .chain(tiles.filter(|tile| Some(*tile) != speaker_camera))
            .filter(|tile| is_visible(tile) && *tile != focus)
            .collect();
        Some(Self { focus, strip })
    }

    /// Sizes are even so tile edges fall on 4:2:0 chroma sample boundaries.
    fn rects(&self, settings: &CompositeSettings) -> Vec<(usize, Rect)> {
        if self.strip.is_empty() {
            return vec![(
                self.focus,
                Rect {
                    x: 0,
                    y: 0,
                    width: settings.width,
                    height: settings.height,
                },
            )];
        }
        let strip_width = (settings.width / STRIP_FRACTION) & !1;
        let tile_height =
            (strip_width * 9 / 16).min(settings.height / self.strip.len() as u32) & !1;
        let focus = Rect {
            x: 0,
            y: 0,
            width: settings.width - strip_width,
            height: settings.height,
        };
        let strip = self.strip.iter().enumerate().map(|(slot, tile)| {
            let rect = Rect {
                x: settings.width - strip_width,
                y: slot as u32 * tile_height,
                width: strip_width,
                height: tile_height,
            };
            (*tile, rect)
        });
        std::iter::once((self.focus, focus)).chain(strip).collect()
    }
}

/// A box the picture is scaled into, keeping its aspect ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ms;

    fn span(start_ms: u64, end_ms: u64) -> Span {
        (ms(start_ms), ms(end_ms))
    }

    fn rect(x: u32, y: u32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn camera(owner: usize) -> Source {
        Source {
            owner,
            content: Content::Camera,
        }
    }

    fn share(owner: usize) -> Source {
        Source {
            owner,
            content: Content::ScreenShare,
        }
    }

    #[test]
    fn the_speaker_takes_the_focus_and_the_others_the_strip() {
        // Arrange: three participants, input 1 speaks
        let sources = [camera(0), camera(1), camera(2)];

        // Act
        let stage = Stage::choose(&sources, &[true, true, true], Some(1), Some(0)).unwrap();

        // Assert
        assert_eq!(
            stage,
            Stage {
                focus: 1,
                strip: vec![0, 2]
            }
        );
        assert_eq!(
            stage.rects(&CompositeSettings::default()),
            [
                (1, rect(0, 0, 1024, 720)),
                (0, rect(1024, 0, 256, 144)),
                (2, rect(1024, 144, 256, 144)),
            ]
        );
    }

    #[test]
    fn the_focus_stays_while_the_speaker_has_no_picture() {
        // Arrange: input 1 speaks with the camera off, input 2 was in focus
        let sources = [camera(0), camera(1), camera(2)];

        // Act
        let stage = Stage::choose(&sources, &[true, false, true], Some(1), Some(2)).unwrap();

        // Assert
        assert_eq!(
            stage,
            Stage {
                focus: 2,
                strip: vec![0]
            }
        );
    }

    #[test]
    fn without_a_speaker_the_first_visible_tile_takes_the_focus() {
        // Arrange
        let sources = [camera(0), camera(1), camera(2)];

        // Act
        let stage = Stage::choose(&sources, &[false, true, true], None, Some(0)).unwrap();

        // Assert
        assert_eq!(
            stage,
            Stage {
                focus: 1,
                strip: vec![2]
            }
        );
    }

    #[test]
    fn nothing_visible_shows_no_stage() {
        // Act / Assert
        assert_eq!(
            Stage::choose(&[camera(0), camera(1)], &[false, false], Some(0), None),
            None
        );
    }

    #[test]
    fn a_screen_share_takes_the_focus_with_the_speaker_atop_the_strip() {
        // Arrange: input 0 shares the screen while input 2 speaks
        let sources = [camera(0), share(0), camera(1), camera(2)];

        // Act
        let stage = Stage::choose(&sources, &[true; 4], Some(2), Some(3)).unwrap();

        // Assert
        assert_eq!(
            stage,
            Stage {
                focus: 1,
                strip: vec![3, 0, 2]
            }
        );
    }

    #[test]
    fn a_later_screen_share_waits_in_the_strip() {
        // Arrange: input 1 shares while input 0's share is in focus
        let sources = [camera(0), share(0), camera(1), share(1)];

        // Act
        let stage = Stage::choose(&sources, &[true; 4], None, Some(1)).unwrap();

        // Assert
        assert_eq!(
            stage,
            Stage {
                focus: 1,
                strip: vec![0, 2, 3]
            }
        );
    }

    #[test]
    fn the_speaker_takes_the_focus_back_when_the_share_ends() {
        // Arrange
        let sources = [camera(0), share(0), camera(1)];

        // Act
        let stage = Stage::choose(&sources, &[true, false, true], Some(1), Some(1)).unwrap();

        // Assert
        assert_eq!(
            stage,
            Stage {
                focus: 2,
                strip: vec![0]
            }
        );
    }

    #[test]
    fn a_single_tile_fills_the_output() {
        // Arrange
        let stage = Stage {
            focus: 0,
            strip: Vec::new(),
        };

        // Act / Assert
        assert_eq!(
            stage.rects(&CompositeSettings::default()),
            [(0, rect(0, 0, 1280, 720))]
        );
    }

    #[test]
    fn a_long_strip_shrinks_its_tiles_to_the_output_height() {
        // Arrange: eight tiles in the strip, 90 px each in 720 px
        let stage = Stage {
            focus: 0,
            strip: (1..=8).collect(),
        };

        // Act
        let rects = stage.rects(&CompositeSettings::default());

        // Assert
        assert_eq!(rects[1], (1, rect(1024, 0, 256, 90)));
        assert_eq!(rects[8], (8, rect(1024, 630, 256, 90)));
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
