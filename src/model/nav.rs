/// A top-level destination in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Destination {
    /// The inbox.
    Inbox,
    /// The current workspace.
    Now,
    /// Recent changes.
    Changes,
    /// Plans.
    Plans,
    /// Agents.
    Agents,
    /// Items to review.
    Review,
    /// The timeline.
    Timeline,
    /// Usage.
    Usage,
    /// Settings.
    Settings,
}

impl Destination {
    /// Destinations in navigation order.
    pub const ALL: [Destination; 9] = [
        Self::Inbox,
        Self::Now,
        Self::Changes,
        Self::Plans,
        Self::Agents,
        Self::Review,
        Self::Timeline,
        Self::Usage,
        Self::Settings,
    ];
    /// The display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Inbox => "Inbox",
            Self::Now => "Now",
            Self::Changes => "Changes",
            Self::Plans => "Plans",
            Self::Agents => "Agents",
            Self::Review => "Review",
            Self::Timeline => "Timeline",
            Self::Usage => "Usage",
            Self::Settings => "Settings",
        }
    }
    /// The storage and identifier slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Now => "now",
            Self::Changes => "changes",
            Self::Plans => "plans",
            Self::Agents => "agents",
            Self::Review => "review",
            Self::Timeline => "timeline",
            Self::Usage => "usage",
            Self::Settings => "settings",
        }
    }
    /// Finds a destination by its slug.
    pub fn from_slug(slug: &str) -> Option<Destination> {
        Self::ALL.into_iter().find(|item| item.slug() == slug)
    }
    /// The destination's keyboard shortcut.
    pub const fn key(self) -> char {
        match self {
            Self::Inbox => 'i',
            Self::Now => 'n',
            Self::Changes => '.',
            Self::Plans => 'p',
            Self::Agents => 'a',
            Self::Review => 'r',
            Self::Timeline => 't',
            Self::Usage => 'u',
            Self::Settings => ',',
        }
    }
    /// Finds a destination by its keyboard shortcut.
    pub fn from_key(key: char) -> Option<Destination> {
        Self::ALL.into_iter().find(|item| item.key() == key)
    }
    /// The phone tab containing this destination.
    pub const fn phone_tab(self) -> PhoneTab {
        match self {
            Self::Inbox => PhoneTab::Inbox,
            Self::Now => PhoneTab::Now,
            Self::Agents => PhoneTab::Agents,
            Self::Plans => PhoneTab::Plans,
            _ => PhoneTab::More,
        }
    }
}

/// A section of settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsSection {
    /// Device settings.
    Devices,
    /// Machine settings.
    Machines,
    /// Connection settings.
    Connection,
    /// Notification settings.
    Notifications,
    /// Assistant settings.
    Assistant,
    /// Autopilot and grants settings.
    Autopilot,
    /// General settings.
    General,
}
impl SettingsSection {
    /// Settings sections in navigation order.
    pub const ALL: [SettingsSection; 7] = [
        Self::Devices,
        Self::Machines,
        Self::Connection,
        Self::Notifications,
        Self::Assistant,
        Self::Autopilot,
        Self::General,
    ];
    /// The display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Devices => "Devices",
            Self::Machines => "Machines",
            Self::Connection => "Connection",
            Self::Notifications => "Notifications",
            Self::Assistant => "Assistant",
            Self::Autopilot => "Autopilot & grants",
            Self::General => "General",
        }
    }
    /// The storage and identifier slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Devices => "devices",
            Self::Machines => "machines",
            Self::Connection => "connection",
            Self::Notifications => "notifications",
            Self::Assistant => "assistant",
            Self::Autopilot => "autopilot",
            Self::General => "general",
        }
    }
    /// Finds a section by its slug.
    pub fn from_slug(slug: &str) -> Option<SettingsSection> {
        Self::ALL.into_iter().find(|item| item.slug() == slug)
    }
}

/// A phone navigation tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhoneTab {
    /// The inbox tab.
    Inbox,
    /// The current workspace tab.
    Now,
    /// The agents tab.
    Agents,
    /// The plans tab.
    Plans,
    /// The remaining destinations.
    More,
}
impl PhoneTab {
    /// Phone tabs in display order.
    pub const ALL: [PhoneTab; 5] = [
        Self::Inbox,
        Self::Now,
        Self::Agents,
        Self::Plans,
        Self::More,
    ];
    /// Destinations listed under More.
    pub const MORE: [Destination; 5] = [
        Destination::Changes,
        Destination::Review,
        Destination::Timeline,
        Destination::Usage,
        Destination::Settings,
    ];
    /// The display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Inbox => "Inbox",
            Self::Now => "Now",
            Self::Agents => "Agents",
            Self::Plans => "Plans",
            Self::More => "More",
        }
    }
    /// The identifier slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Now => "now",
            Self::Agents => "agents",
            Self::Plans => "plans",
            Self::More => "more",
        }
    }
    /// The tab's zero-based index.
    pub const fn index(self) -> u32 {
        match self {
            Self::Inbox => 0,
            Self::Now => 1,
            Self::Agents => 2,
            Self::Plans => 3,
            Self::More => 4,
        }
    }
    /// Finds a tab by its zero-based index.
    pub fn from_index(index: u32) -> Option<PhoneTab> {
        Self::ALL.into_iter().find(|item| item.index() == index)
    }
    /// The tab's destination, if it opens one directly.
    pub const fn destination(self) -> Option<Destination> {
        match self {
            Self::Inbox => Some(Destination::Inbox),
            Self::Now => Some(Destination::Now),
            Self::Agents => Some(Destination::Agents),
            Self::Plans => Some(Destination::Plans),
            Self::More => None,
        }
    }
}

/// An overlay above the current destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Overlay {
    /// The Ask composer.
    Ask,
    /// The command palette.
    Palette,
}
/// The recipient selected in Ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AskTarget {
    /// The assistant.
    Assistant,
    /// The PM.
    Pm,
}
impl AskTarget {
    /// The display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Assistant => "Assistant",
            Self::Pm => "PM",
        }
    }
    /// The identifier slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::Pm => "pm",
        }
    }
}
/// The selected appearance preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeChoice {
    /// Follow the system appearance.
    System,
    /// Use the light appearance.
    Light,
    /// Use the dark appearance.
    Dark,
}
impl ThemeChoice {
    /// The display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
    /// The storage slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
    /// Finds an appearance preference by its slug.
    pub fn from_slug(slug: &str) -> Option<ThemeChoice> {
        match slug {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}
/// The app's presentation form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// Desktop and web presentation.
    Desktop,
    /// Phone presentation.
    Phone,
    /// Terminal presentation.
    Terminal,
}
impl Form {
    /// The initial destination for this form.
    pub const fn home(self) -> Destination {
        match self {
            Self::Phone => Destination::Inbox,
            Self::Desktop | Self::Terminal => Destination::Now,
        }
    }
}

/// Navigation state shared across presentation forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavState {
    /// The current destination.
    pub destination: Destination,
    /// The current settings section.
    pub settings: SettingsSection,
    /// Open overlays in oldest-first order.
    pub overlays: Vec<Overlay>,
}
impl NavState {
    /// Creates navigation state at the form's home destination.
    pub fn new(form: Form) -> NavState {
        NavState {
            destination: form.home(),
            settings: SettingsSection::Devices,
            overlays: Vec::new(),
        }
    }
    /// Navigates to a destination and closes the palette.
    pub fn go(&mut self, destination: Destination) {
        self.destination = destination;
        self.close(Overlay::Palette);
    }
    /// Navigates to a settings section and closes the palette.
    pub fn go_settings(&mut self, section: SettingsSection) {
        self.settings = section;
        self.go(Destination::Settings);
    }
    /// Opens an overlay or moves it to the newest position.
    pub fn open(&mut self, overlay: Overlay) {
        self.close(overlay);
        self.overlays.push(overlay);
    }
    /// Closes an overlay if open.
    pub fn close(&mut self, overlay: Overlay) {
        self.overlays.retain(|open| *open != overlay);
    }
    /// Closes and returns the newest overlay.
    pub fn close_newest(&mut self) -> Option<Overlay> {
        self.overlays.pop()
    }
    /// Whether the overlay is open.
    pub fn is_open(&self, overlay: Overlay) -> bool {
        self.overlays.contains(&overlay)
    }
    /// Returns the newest open overlay.
    pub fn newest(&self) -> Option<Overlay> {
        self.overlays.last().copied()
    }
    /// Handles a destination or Ask key unless an overlay is open.
    pub fn press_key(&mut self, key: char) -> bool {
        if !self.overlays.is_empty() {
            return false;
        }
        if key == '/' {
            self.open(Overlay::Ask);
            true
        } else if let Some(destination) = Destination::from_key(key) {
            self.go(destination);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_keys_round_trip() {
        for destination in Destination::ALL {
            assert_eq!(Destination::from_key(destination.key()), Some(destination));
        }
    }
    #[test]
    fn keys_are_ignored_while_overlay_is_open() {
        let mut nav = NavState::new(Form::Desktop);
        nav.open(Overlay::Ask);
        let before = nav.clone();
        assert!(!nav.press_key('i'));
        assert!(!nav.press_key('/'));
        assert_eq!(nav, before);
    }
    #[test]
    fn newest_overlay_closes_first() {
        let mut nav = NavState::new(Form::Desktop);
        nav.open(Overlay::Ask);
        nav.open(Overlay::Palette);
        nav.open(Overlay::Ask);
        assert_eq!(nav.close_newest(), Some(Overlay::Ask));
        assert_eq!(nav.close_newest(), Some(Overlay::Palette));
        assert_eq!(nav.close_newest(), None);
    }
}
