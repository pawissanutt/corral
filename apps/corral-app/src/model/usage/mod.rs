//! Usage fixtures and projections for the desktop table and stacked phone blocks.
//! All costs and quota numbers are illustrative; usage is visibility only.

use crate::model::Effect;

/// The selected period; the illustrative fixture remains unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Period {
    /// Today's usage.
    #[default]
    Today,
    /// Seven-day period.
    Week,
    /// Thirty-day period.
    Month,
}

impl Period {
    /// Period selector options in display order.
    pub const ALL: [Period; 3] = [Period::Today, Period::Week, Period::Month];

    /// Label shown in the period selector.
    pub fn label(self) -> &'static str {
        match self {
            Period::Today => "Today",
            Period::Week => "7 days",
            Period::Month => "30 days",
        }
    }
}

/// Selection state for the illustrative usage screen.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    /// Currently selected period.
    pub period: Period,
}

impl State {
    /// Select a period without changing the illustrative data or emitting effects.
    pub fn select_period(&mut self, period: Period) -> Vec<Effect> {
        self.period = period;
        Vec::new()
    }
}

/// A profile's share of its provider window and today's cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Profile {
    /// Profile or quick-thread name.
    pub name: &'static str,
    /// Fraction of the provider window consumed by this profile.
    pub used: f32,
    /// Today's illustrative cost in dollars.
    pub today: f32,
}

/// One provider's current window, costs, profiles and today's usage history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Provider {
    /// Connection identifier.
    pub id: &'static str,
    /// Provider heading.
    pub label: &'static str,
    /// Window description.
    pub window: &'static str,
    /// Fraction of the current window used.
    pub used: f32,
    /// Reset time or duration as supplied by the fixture.
    pub resets: &'static str,
    /// Optional fraction of the weekly cap used.
    pub weekly: Option<f32>,
    /// Today's cost in dollars.
    pub today: f32,
    /// Seven-day cost in dollars.
    pub week: f32,
    /// Profiles in table and phone order.
    pub profiles: &'static [Profile],
    /// Window use sampled at [`HOURS`].
    pub series: &'static [f32],
}

/// The illustrative provider and profile records.
pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "openai-subscription",
        label: "OpenAI · subscription",
        window: "5 h rolling",
        used: 0.78,
        resets: "1h 46m",
        weekly: Some(0.61),
        today: 61.2,
        week: 318.4,
        profiles: &[
            Profile {
                name: "pi-sol xhigh",
                used: 0.69,
                today: 53.7,
            },
            Profile {
                name: "pi-terra high",
                used: 0.07,
                today: 6.4,
            },
            Profile {
                name: "assistant · quick thread A-7",
                used: 0.02,
                today: 1.1,
            },
        ],
        series: &[
            0.12, 0.2, 0.31, 0.38, 0.42, 0.47, 0.55, 0.61, 0.66, 0.72, 0.75, 0.78,
        ],
    },
    Provider {
        id: "anthropic-subscription",
        label: "Anthropic · subscription",
        window: "5 h rolling",
        used: 0.34,
        resets: "3h 02m",
        weekly: Some(0.48),
        today: 22.7,
        week: 141.0,
        profiles: &[
            Profile {
                name: "omo opus",
                used: 0.29,
                today: 19.1,
            },
            Profile {
                name: "claude sonnet review",
                used: 0.05,
                today: 3.6,
            },
        ],
        series: &[
            0.05, 0.08, 0.1, 0.18, 0.22, 0.25, 0.27, 0.29, 0.3, 0.31, 0.33, 0.34,
        ],
    },
    Provider {
        id: "openai-api",
        label: "OpenAI · API (metered)",
        window: "allowance $40 / day",
        used: 0.21,
        resets: "01:48",
        weekly: None,
        today: 8.4,
        week: 29.9,
        profiles: &[Profile {
            name: "fallback · pi-sol",
            used: 0.21,
            today: 8.4,
        }],
        series: &[
            0.0, 0.0, 0.0, 0.0, 0.04, 0.04, 0.09, 0.12, 0.15, 0.18, 0.2, 0.21,
        ],
    },
];

/// Hour labels corresponding to the provider series.
pub const HOURS: &[&str] = &[
    "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22",
];

/// One day's costs in the order of [`COST_LEGEND`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayCost {
    /// Day abbreviation.
    pub day: &'static str,
    /// Dollar amounts for the three cost categories.
    pub values: [u32; 3],
}

/// The seven illustrative daily cost records.
pub const WEEK: &[DayCost] = &[
    DayCost {
        day: "Thu",
        values: [38, 21, 2],
    },
    DayCost {
        day: "Fri",
        values: [52, 18, 4],
    },
    DayCost {
        day: "Sat",
        values: [12, 9, 0],
    },
    DayCost {
        day: "Sun",
        values: [44, 26, 5],
    },
    DayCost {
        day: "Mon",
        values: [57, 30, 6],
    },
    DayCost {
        day: "Tue",
        values: [54, 14, 4],
    },
    DayCost {
        day: "Wed",
        values: [61, 23, 8],
    },
];
/// Cost bar segment labels, in the same order as [`DayCost::values`].
pub const COST_LEGEND: [&str; 3] = ["OpenAI sub", "Anthropic sub", "metered API"];
/// Usage page title.
pub const TITLE: &str = "Usage";
/// Phone back destination label.
pub const BACK_LABEL: &str = "More";
/// Desktop page subtitle.
pub const SUBTITLE: &str = "cost and quota per provider and profile · visibility only, no routing";
/// Period selector labels; Today is selected in the mockup.
pub const PERIODS: [&str; 3] = ["Today", "7 days", "30 days"];
/// The small quiet word following a provider cell's desktop usage percentage.
pub const USED_LABEL: &str = "used";
/// Profile table heading.
pub const PROFILE_TITLE: &str = "By profile · current window";
/// Profile table column labels in order.
pub const PROFILE_HEADERS: [&str; 6] = [
    "Provider",
    "Profile",
    "Window",
    "Share · 80% marker",
    "Today",
    "Headroom",
];
/// The share meter's marker, as a percentage.
pub const SHARE_MARKER_PERCENT: u32 = 80;
/// Daily cost section heading.
pub const COST_TITLE: &str = "Cost per day · 7 days";
/// Desktop illustration disclaimer.
pub const DESKTOP_DISCLAIMER: &str = "All numbers illustrative. Connection names follow real usage records (openai-subscription); bands mark 70% and 90% of the provider window.";
/// Phone illustration disclaimer.
pub const PHONE_DISCLAIMER: &str = "All numbers illustrative.";

/// Usage band for a provider or a profile meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Below 70 percent.
    None,
    /// At least 70 percent but below 90 percent.
    Warn,
    /// At least 90 percent.
    Bad,
}

/// Resolve the window use thresholds used by the mockup.
pub fn level(used: f32) -> Level {
    if used >= 0.9 {
        Level::Bad
    } else if used >= 0.7 {
        Level::Warn
    } else {
        Level::None
    }
}

fn percent(value: f32) -> u32 {
    (value * 100.0).round() as u32
}
fn money(value: f32) -> String {
    if value >= 10.0 {
        format!("${:.0}", value.round())
    } else {
        format!("${value:.2}")
    }
}

/// A coordinate in the chart's supplied view box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal position.
    pub x: f32,
    /// Vertical position.
    pub y: f32,
}
/// A coloured threshold rectangle in the chart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    /// Threshold band represented by this rectangle.
    pub level: Level,
    /// Top-left position.
    pub origin: Point,
    /// Rectangle width.
    pub width: f32,
    /// Rectangle height.
    pub height: f32,
}
/// A chart axis label and its baseline position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisLabel {
    /// Label text.
    pub text: &'static str,
    /// Text position; hour labels are centered on x.
    pub position: Point,
}
/// Geometric chart data, including the filled-area polygon and final point.
#[derive(Debug, Clone, PartialEq)]
pub struct BandChart {
    /// 90 to 100 and 70 to 90 percent zones, in that order.
    pub zones: [Zone; 2],
    /// Vertical band and zero labels.
    pub vertical_labels: [AxisLabel; 3],
    /// Every third hour label.
    pub hour_labels: Vec<AxisLabel>,
    /// Usage series in sample order.
    pub line: Vec<Point>,
    /// Series closed to the zero baseline for an area renderer.
    pub area: Vec<Point>,
    /// Last sample, if the series is nonempty.
    pub last: Option<Point>,
    /// Accessible chart description.
    pub accessibility_label: String,
}

/// Project a provider's samples and threshold bands into the requested chart dimensions.
pub fn band_chart(provider: &Provider, width: f32, height: f32) -> BandChart {
    let plot_width = width - 26.0;
    let x = |i: usize| {
        22.0 + if provider.series.len() > 1 {
            i as f32 / (provider.series.len() - 1) as f32 * plot_width
        } else {
            0.0
        }
    };
    let y = |v: f32| 4.0 + (1.0 - v) * (height - 26.0);
    let zones = [
        Zone {
            level: Level::Bad,
            origin: Point { x: 22.0, y: y(1.0) },
            width: plot_width,
            height: y(0.9) - y(1.0),
        },
        Zone {
            level: Level::Warn,
            origin: Point { x: 22.0, y: y(0.9) },
            width: plot_width,
            height: y(0.7) - y(0.9),
        },
    ];
    let vertical_labels = [
        AxisLabel {
            text: "90",
            position: Point {
                x: 0.0,
                y: y(0.95) + 3.0,
            },
        },
        AxisLabel {
            text: "70",
            position: Point {
                x: 0.0,
                y: y(0.8) + 3.0,
            },
        },
        AxisLabel {
            text: "0",
            position: Point { x: 0.0, y: y(0.0) },
        },
    ];
    let hour_labels = HOURS
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 3 == 0)
        .map(|(i, &text)| AxisLabel {
            text,
            position: Point {
                x: x(i),
                y: height - 2.0,
            },
        })
        .collect();
    let line: Vec<Point> = provider
        .series
        .iter()
        .enumerate()
        .map(|(i, &v)| Point { x: x(i), y: y(v) })
        .collect();
    let last = line.last().copied();
    let mut area = line.clone();
    if let (Some(first), Some(end)) = (line.first(), last) {
        area.push(Point {
            x: end.x,
            y: y(0.0),
        });
        area.push(Point {
            x: first.x,
            y: y(0.0),
        });
    }
    BandChart {
        zones,
        vertical_labels,
        hour_labels,
        line,
        area,
        last,
        accessibility_label: format!(
            "{} window use today, {} percent now",
            provider.label,
            percent(provider.used)
        ),
    }
}

/// One provider cell, also used as the basis of a stacked phone block.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderView {
    /// Source record including identity and window.
    pub provider: &'static Provider,
    /// Current window use, including the percent sign.
    pub used: String,
    /// Headroom including its percent sign and label.
    pub headroom: String,
    /// Provider threshold, used for headroom and profile meters.
    pub level: Level,
    /// Desktop reset sentence.
    pub resets: String,
    /// Phone reset sentence, which omits at/in.
    pub phone_resets: String,
    /// Desktop cost and optional weekly-cap line.
    pub costs: String,
    /// Phone profile rows in order.
    pub profiles: Vec<PhoneProfile>,
}
/// A profile line inside a phone provider block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneProfile {
    /// Profile name.
    pub name: &'static str,
    /// Window percentage and today's cost.
    pub detail: String,
}
/// Build the cells in provider order; phone renders these as stacked blocks.
pub fn providers() -> Vec<ProviderView> {
    PROVIDERS
        .iter()
        .map(|p| {
            let weekly = p
                .weekly
                .map(|w| format!(" · weekly cap {}%", percent(w)))
                .unwrap_or_default();
            ProviderView {
                provider: p,
                used: format!("{}%", percent(p.used)),
                headroom: format!("{}% headroom", percent(1.0 - p.used)),
                level: level(p.used),
                resets: format!(
                    "resets {} {}",
                    if p.resets.contains(':') { "at" } else { "in" },
                    p.resets
                ),
                phone_resets: format!("resets {}", p.resets),
                costs: format!(
                    "today {} · 7 days {}{}",
                    money(p.today),
                    money(p.week),
                    weekly
                ),
                profiles: p
                    .profiles
                    .iter()
                    .map(|profile| PhoneProfile {
                        name: profile.name,
                        detail: format!("{}% · {}", percent(profile.used), money(profile.today)),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// One desktop profile table row, with repeated provider and headroom cells suppressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    /// Provider label on the first row only.
    pub provider: &'static str,
    /// Profile name.
    pub profile: &'static str,
    /// Profile window percentage.
    pub window: String,
    /// Meter fill width as a percentage.
    pub share_percent: u32,
    /// Meter level follows provider use, not profile use.
    pub level: Level,
    /// Today's profile cost.
    pub today: String,
    /// Provider headroom on the first row only.
    pub headroom: String,
}
/// Flatten all providers' profiles, including the quick thread, into table rows.
pub fn profile_rows() -> Vec<ProfileRow> {
    PROVIDERS
        .iter()
        .flat_map(|p| {
            p.profiles
                .iter()
                .enumerate()
                .map(move |(i, profile)| ProfileRow {
                    provider: if i == 0 { p.label } else { "" },
                    profile: profile.name,
                    window: format!("{}%", percent(profile.used)),
                    share_percent: percent(profile.used),
                    level: level(p.used),
                    today: money(profile.today),
                    headroom: if i == 0 {
                        format!("{}%", percent(1.0 - p.used))
                    } else {
                        String::new()
                    },
                })
        })
        .collect()
}

/// One stacked daily cost bar, with segment widths in percent and a dollar total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostBar {
    /// Day abbreviation.
    pub day: &'static str,
    /// Segment values and widths, in legend order.
    pub values: [u32; 3],
    /// Sum of the three dollar values.
    pub total: u32,
    /// Formatted sum shown next to the bar.
    pub total_label: String,
}
/// Build the seven daily bars in fixture order.
pub fn cost_bars() -> Vec<CostBar> {
    WEEK.iter()
        .map(|day| {
            let total = day.values.iter().sum();
            CostBar {
                day: day.day,
                values: day.values,
                total,
                total_label: format!("${total}"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Level, level};

    #[test]
    fn usage_thresholds_do_not_shift_at_band_edges() {
        assert_eq!(level(0.69), Level::None);
        assert_eq!(level(0.70), Level::Warn);
        assert_eq!(level(0.90), Level::Bad);
    }
}
