//! The mockup's illustrative providers, profiles and daily costs, shown only in the explicit
//! fixture preview.

/// A profile's share of its provider window and today's cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Profile {
    pub name: &'static str,
    /// Fraction of the provider window consumed by this profile.
    pub used: f32,
    /// Today's cost in dollars.
    pub today: f32,
}

/// One provider's current window, costs, profiles and today's usage history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Provider {
    pub id: &'static str,
    pub label: &'static str,
    pub window: &'static str,
    /// Fraction of the current window used.
    pub used: f32,
    /// Reset time or duration.
    pub resets: &'static str,
    /// Fraction of the weekly cap used.
    pub weekly: Option<f32>,
    pub today: f32,
    pub week: f32,
    pub profiles: &'static [Profile],
    /// Window use sampled at [`HOURS`].
    pub series: &'static [f32],
}

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

/// The seven illustrative days and their dollar amounts per [`COST_LEGEND`] entry.
pub const WEEK: &[(&str, [u32; 3])] = &[
    ("Thu", [38, 21, 2]),
    ("Fri", [52, 18, 4]),
    ("Sat", [12, 9, 0]),
    ("Sun", [44, 26, 5]),
    ("Mon", [57, 30, 6]),
    ("Tue", [54, 14, 4]),
    ("Wed", [61, 23, 8]),
];
/// Cost bar segment labels, in the same order as [`WEEK`]'s values.
pub const COST_LEGEND: [&str; 3] = ["OpenAI sub", "Anthropic sub", "metered API"];
