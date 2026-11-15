//! Destinations, pages and overlays shared by every shell.

use crate::model::live::{Live, Route, Selection};

/// Desktop sidebar prompt for the Ask composer.
pub const ASK_HINT: &str = "Ask…";
/// Phone floating action label for the Ask composer.
pub const ASK_LABEL: &str = "Ask";
/// Desktop sidebar prompt for the command palette.
pub const PALETTE_HINT: &str = "Go to, answer, open…";
/// Compact terminal palette launcher label.
pub const PALETTE_LABEL: &str = "Go to";
/// The sidebar's name in the terminal key line, after its Ctrl+B cap.
pub const SIDEBAR_LABEL: &str = "Sidebar";
/// Global Ask shortcut displayed by shell chrome.
pub const ASK_KEY: &str = "/";
/// Global palette shortcut displayed by shell chrome.
pub const PALETTE_KEY: &str = "Ctrl K";
/// Device identity prefix in the desktop sidebar footer.
pub const THIS_DEVICE: &str = "This device";
/// Command attribution prefix in the desktop sidebar footer.
pub const SIGNS_AS: &str = "Actions sign as";

/// Label of the phone's floating Ask button.
pub const ASK_BUTTON: &str = "Ask";
/// Accessible action name of the phone's floating Ask button.
pub const ASK_BUTTON_LABEL: &str = "Ask (opens the composer)";
/// Accessible label of the phone large-title palette search button.
pub const SEARCH_LABEL: &str = "Search";

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
    /// Sidebar badges use the same canonical owner moves as their destination groups.
    pub fn attention_count(self, counts: &corral_protocol::rows::AttentionCounts) -> Option<u64> {
        match self {
            Self::Inbox => Some(counts.inbox),
            Self::Now => Some(counts.now),
            Self::Plans => Some(counts.plans),
            Self::Review => Some(counts.review),
            _ => None,
        }
    }
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
/// The desktop and terminal sidebar's wide-layout choice. A narrow window or grid always folds
/// the sidebar to its rail whatever the choice, so a choice made wide never crushes a narrow
/// layout; there the toggle opens the labels over the content for the moment instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SidebarPref {
    /// Labels while wide; the user has not chosen.
    #[default]
    Auto,
    /// Labels while wide, by choice.
    Expanded,
    /// The rail even while wide, by choice.
    Collapsed,
}
impl SidebarPref {
    /// The storage slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Expanded => "expanded",
            Self::Collapsed => "collapsed",
        }
    }
    /// Finds a preference by its slug.
    pub fn from_slug(slug: &str) -> Option<SidebarPref> {
        [Self::Auto, Self::Expanded, Self::Collapsed]
            .into_iter()
            .find(|pref| pref.slug() == slug)
    }
    /// Whether the layout gives the sidebar only its rail at this width: always while narrow.
    pub const fn collapsed(self, narrow: bool) -> bool {
        narrow || matches!(self, Self::Collapsed)
    }
    /// The wide choice that flips what shows wide; it stays put for the return to wide when
    /// the toggle is used while narrow.
    pub const fn toggled(self, narrow: bool) -> SidebarPref {
        if narrow {
            self
        } else if self.collapsed(false) {
            Self::Expanded
        } else {
            Self::Collapsed
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
    /// Counts explicit section-opening intents, including a repeated opening of the same section.
    pub section_opens: u32,
    /// Grows by one (wrapping) on every `go`, `go_settings` included, so a shell can tell a
    /// navigation to the destination it already shows from no change.
    pub goes: u32,
    /// Open overlays in oldest-first order.
    pub overlays: Vec<Overlay>,
    /// Open pages in oldest-first order; task, plan and review pages are phone-only.
    pub pages: Vec<Route>,
    /// The live record selected in each domain.
    pub selection: Selection,
}
impl NavState {
    /// Creates navigation state at the form's home destination.
    pub fn new(form: Form) -> NavState {
        NavState {
            destination: form.home(),
            settings: SettingsSection::Devices,
            section_opens: 0,
            goes: 0,
            overlays: Vec::new(),
            pages: Vec::new(),
            selection: Selection::default(),
        }
    }
    /// Navigates to a destination and closes the palette.
    pub fn go(&mut self, destination: Destination) {
        self.goes = self.goes.wrapping_add(1);
        self.destination = destination;
        self.pages.clear();
        self.close(Overlay::Palette);
    }
    /// Navigates to a settings section and closes the palette.
    pub fn go_settings(&mut self, section: SettingsSection) {
        self.settings = section;
        self.section_opens = self.section_opens.wrapping_add(1);
        self.go(Destination::Settings);
    }
    /// Selects a live record, then pushes its page where the form stacks it; tasks, plans and
    /// reviews select in place off phone.
    pub fn open_live(&mut self, live: Live, form: Form) {
        self.selection.set(&live);
        let push = live.pushes(form);
        self.open_route(Route::Live(live), push);
    }
    fn open_route(&mut self, route: Route, push: bool) {
        self.destination = route.owner();
        self.close(Overlay::Palette);
        if push {
            if self.pages.last() != Some(&route) {
                self.pages.push(route);
            }
        } else {
            self.pages.clear();
        }
    }
    /// Pops the top page; its owner remains selected when the stack becomes empty.
    pub fn back(&mut self) -> bool {
        let Some(route) = self.pages.pop() else {
            return false;
        };
        self.destination = self.pages.last().unwrap_or(&route).owner();
        true
    }
    /// The live record whose page is currently above the destination.
    pub fn live(&self) -> Option<&Live> {
        match self.pages.last() {
            Some(Route::Live(live)) => Some(live),
            _ => None,
        }
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

    #[test]
    fn live_records_select_everywhere_and_push_only_where_the_form_stacks_them() {
        use crate::model::id::Id;
        for form in [Form::Desktop, Form::Terminal, Form::Phone] {
            let mut nav = NavState::new(form);
            let task = Live::Task(Id::from("T-9"));
            nav.open_live(task.clone(), form);
            assert_eq!(nav.destination, Destination::Now);
            assert_eq!(nav.selection.task.as_deref(), Some("T-9"));
            assert_eq!(nav.live(), (form == Form::Phone).then_some(&task));
            let question = Live::Question(Id::from("T-9/R1/Q1"));
            nav.open_live(question.clone(), form);
            nav.open_live(question.clone(), form);
            assert_eq!(nav.live(), Some(&question));
            assert_eq!(nav.pages.len(), if form == Form::Phone { 2 } else { 1 });
            assert!(nav.back());
            assert_eq!(nav.selection.question.as_deref(), Some("T-9/R1/Q1"));
        }
    }

    #[test]
    fn repeated_settings_section_opens_remain_observable() {
        let mut nav = NavState::new(Form::Phone);
        nav.go_settings(SettingsSection::Devices);
        let first = nav.clone();
        nav.go_settings(SettingsSection::Devices);
        assert_ne!(nav, first);
        assert_eq!(nav.section_opens, first.section_opens.wrapping_add(1));
        let opens = nav.section_opens;
        nav.go(Destination::Settings);
        assert_eq!(nav.section_opens, opens);
        nav.section_opens = u32::MAX;
        nav.go_settings(SettingsSection::Devices);
        assert_eq!(nav.section_opens, 0);
    }

    #[test]
    fn navigating_to_the_current_destination_remains_observable() {
        let mut nav = NavState::new(Form::Phone);
        let first = nav.clone();
        nav.go(nav.destination);
        assert_ne!(nav, first);
        assert_eq!(nav.goes, first.goes.wrapping_add(1));
        let goes = nav.goes;
        nav.go_settings(SettingsSection::Devices);
        assert_eq!(nav.goes, goes.wrapping_add(1));
        nav.goes = u32::MAX;
        nav.go(nav.destination);
        assert_eq!(nav.goes, 0);
    }

    #[test]
    fn a_wide_choice_never_unfolds_a_narrow_sidebar_and_survives_the_return_to_wide() {
        let mut pref = SidebarPref::Auto;
        pref = pref.toggled(false).toggled(false);
        assert_eq!(pref, SidebarPref::Expanded);
        // Narrow: folded whatever was chosen wide, and the toggle keeps the wide choice.
        assert!(pref.collapsed(true));
        assert_eq!(pref.toggled(true), SidebarPref::Expanded);
        // Wide again: the choice made wide shows again.
        assert!(!pref.collapsed(false));
        assert!(SidebarPref::Collapsed.collapsed(false));
        for pref in [
            SidebarPref::Auto,
            SidebarPref::Expanded,
            SidebarPref::Collapsed,
        ] {
            assert_eq!(SidebarPref::from_slug(pref.slug()), Some(pref));
        }
    }
}
