#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendition {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub bitrate_kbps: u32,
}

const LADDER: [(u32, u32); 4] = [(1080, 4_500), (720, 2_500), (480, 1_200), (360, 700)];

pub fn ladder_for(source_width: u32, source_height: u32) -> Vec<Rendition> {
    LADDER
        .iter()
        .filter(|(height, _)| *height < source_height)
        .map(|(height, bitrate_kbps)| Rendition {
            name: format!("{height}p"),
            width: (source_width * height / source_height).next_multiple_of(2),
            height: *height,
            bitrate_kbps: *bitrate_kbps,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_renditions_below_a_1080p_source() {
        // Arrange
        let (width, height) = (1920, 1080);

        // Act
        let ladder = ladder_for(width, height);

        // Assert
        let summary: Vec<(&str, u32, u32, u32)> = ladder
            .iter()
            .map(|r| (r.name.as_str(), r.width, r.height, r.bitrate_kbps))
            .collect();
        assert_eq!(
            summary,
            [
                ("720p", 1280, 720, 2_500),
                ("480p", 854, 480, 1_200),
                ("360p", 640, 360, 700)
            ]
        );
    }

    #[test]
    fn never_upscales() {
        // Arrange / Act
        let ladder = ladder_for(640, 360);

        // Assert
        assert!(ladder.is_empty());
    }

    #[test]
    fn keeps_portrait_aspect_ratio_with_even_width() {
        // Arrange / Act
        let ladder = ladder_for(1080, 1920);

        // Assert
        assert_eq!((ladder[0].width, ladder[0].height), (608, 1080));
    }
}
