//! Quick layout presets (`LAYOUT 1|2|2H|3|4|6`); `dock::Node::preset` builds the tree.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Layout {
    One,
    TwoCols,
    TwoRows,
    /// Large pane on the left, two stacked on the right.
    MainRight2,
    Grid2x2,
    Grid3x2,
}

impl Layout {
    pub const ALL: [Layout; 6] =
        [Layout::One, Layout::TwoCols, Layout::TwoRows, Layout::MainRight2, Layout::Grid2x2, Layout::Grid3x2];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_uppercase().as_str() {
            "1" => Layout::One,
            "2" | "2V" | "2C" => Layout::TwoCols,
            "2H" | "2R" => Layout::TwoRows,
            "3" => Layout::MainRight2,
            "4" => Layout::Grid2x2,
            "6" => Layout::Grid3x2,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Layout::One => "1",
            Layout::TwoCols => "2",
            Layout::TwoRows => "2H",
            Layout::MainRight2 => "3",
            Layout::Grid2x2 => "4",
            Layout::Grid3x2 => "6",
        }
    }

    pub fn slots(self) -> usize {
        match self {
            Layout::One => 1,
            Layout::TwoCols | Layout::TwoRows => 2,
            Layout::MainRight2 => 3,
            Layout::Grid2x2 => 4,
            Layout::Grid3x2 => 6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_parse_and_label() {
        for l in Layout::ALL {
            assert_eq!(Layout::parse(l.label()), Some(l));
            assert!(l.slots() >= 1);
        }
        assert_eq!(Layout::parse("2v"), Some(Layout::TwoCols));
        assert_eq!(Layout::parse("Morning"), None);
    }
}
