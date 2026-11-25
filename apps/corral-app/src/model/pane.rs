//! Room for Ask beside the page, without forgetting the user's preferred width.

pub const PANE_LEAST: f32 = 360.0;
pub const PANE_DEFAULT: f32 = 500.0;
pub const PAGE_LEAST: f32 = 480.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    Beside { width: f32, least: f32, most: f32 },
    Over,
}

pub fn placement(content: f32, wanted: f32) -> Placement {
    let most = content - PAGE_LEAST;
    if most < PANE_LEAST {
        Placement::Over
    } else {
        Placement::Beside {
            width: kept(wanted).min(most),
            least: PANE_LEAST,
            most,
        }
    }
}

pub fn kept(width: f32) -> f32 {
    width.max(PANE_LEAST)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_only_docks_when_both_minimums_fit() {
        let threshold = PAGE_LEAST + PANE_LEAST;
        assert_eq!(placement(threshold - 1.0, PANE_DEFAULT), Placement::Over);
        assert_eq!(
            placement(threshold, PANE_DEFAULT),
            Placement::Beside {
                width: PANE_LEAST,
                least: PANE_LEAST,
                most: PANE_LEAST,
            }
        );
    }

    #[test]
    fn temporary_width_limits_do_not_erase_the_saved_preference() {
        let wanted = kept(700.0);
        assert_eq!(kept(200.0), PANE_LEAST);
        assert_eq!(
            placement(900.0, wanted),
            Placement::Beside {
                width: 420.0,
                least: PANE_LEAST,
                most: 420.0,
            }
        );
        assert_eq!(
            placement(1300.0, wanted),
            Placement::Beside {
                width: 700.0,
                least: PANE_LEAST,
                most: 820.0,
            }
        );
    }
}
