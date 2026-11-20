//! The four places, their pages and overlays shared by every shell.

use crate::model::live::{Live, Route, Selection};

/// Phone floating action label for the Ask composer.
pub const ASK_LABEL: &str = "Ask";
/// Desktop sidebar prompt for the command palette.
pub const PALETTE_HINT: &str = "Go to…";
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

/// A page the navigation opens directly: the four places' own pages and Settings. Activity has
/// four pages; every other place has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Destination {
    /// What waits on the owner, then what is working.
    Home,
    /// Every task, grouped by status.
    Tasks,
    /// Every session, grouped by state.
    Sessions,
    /// Activity › Feed: what changed.
    Feed,
    /// Activity › Timeline.
    Timeline,
    /// Activity › Usage.
    Usage,
    /// Activity › Artifacts: the finder over every task's artifacts.
    Artifacts,
    /// Settings.
    Settings,
}

impl Destination {
    /// Destinations in navigation order.
    pub const ALL: [Destination; 8] = [
        Self::Home,
        Self::Tasks,
        Self::Sessions,
        Self::Feed,
        Self::Timeline,
        Self::Usage,
        Self::Artifacts,
        Self::Settings,
    ];
    /// Activity's pages, in the order the sidebar lists them.
    pub const ACTIVITY: [Destination; 4] =
        [Self::Feed, Self::Timeline, Self::Usage, Self::Artifacts];
    /// The display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Tasks => "Tasks",
            Self::Sessions => "Sessions",
            Self::Feed => "Feed",
            Self::Timeline => "Timeline",
            Self::Usage => "Usage",
            Self::Artifacts => "Artifacts",
            Self::Settings => "Settings",
        }
    }
    /// The storage and identifier slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Tasks => "tasks",
            Self::Sessions => "sessions",
            Self::Feed => "feed",
            Self::Timeline => "timeline",
            Self::Usage => "usage",
            Self::Artifacts => "artifacts",
            Self::Settings => "settings",
        }
    }
    /// Finds a destination by its slug. The nine former destinations' slugs open their new
    /// homes, so a stored destination or an old link still lands somewhere.
    pub fn from_slug(slug: &str) -> Option<Destination> {
        match slug {
            "now" | "inbox" => Some(Self::Home),
            "plans" | "review" => Some(Self::Tasks),
            "agents" | "builds" => Some(Self::Sessions),
            "changes" | "activity" => Some(Self::Feed),
            _ => Self::ALL.into_iter().find(|item| item.slug() == slug),
        }
    }
    /// The place the page belongs to.
    pub const fn area(self) -> Area {
        match self {
            Self::Home => Area::Home,
            Self::Tasks => Area::Tasks,
            Self::Sessions => Area::Sessions,
            Self::Feed | Self::Timeline | Self::Usage | Self::Artifacts => Area::Activity,
            Self::Settings => Area::Settings,
        }
    }
    /// The phone tab whose stack shows the page; none for Settings, which opens over the tab
    /// in view.
    pub const fn phone_tab(self) -> Option<PhoneTab> {
        match self.area() {
            Area::Home => Some(PhoneTab::Home),
            Area::Tasks => Some(PhoneTab::Tasks),
            Area::Sessions => Some(PhoneTab::Sessions),
            Area::Activity => Some(PhoneTab::Activity),
            Area::Settings => None,
        }
    }
}

/// One of the four places, or Settings in its corner: what the sidebar lists at rest and a
/// place key opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Area {
    Home,
    Tasks,
    Sessions,
    /// Feed, Timeline, Usage and Artifacts.
    Activity,
    Settings,
}

impl Area {
    /// The four places in navigation order.
    pub const PLACES: [Area; 4] = [Self::Home, Self::Tasks, Self::Sessions, Self::Activity];
    /// The places, then Settings.
    pub const ALL: [Area; 5] = [
        Self::Home,
        Self::Tasks,
        Self::Sessions,
        Self::Activity,
        Self::Settings,
    ];
    /// The display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Tasks => "Tasks",
            Self::Sessions => "Sessions",
            Self::Activity => "Activity",
            Self::Settings => "Settings",
        }
    }
    /// The identifier slug (`nav-<slug>`, `tab-<slug>`).
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Tasks => "tasks",
            Self::Sessions => "sessions",
            Self::Activity => "activity",
            Self::Settings => "settings",
        }
    }
    /// The place's keyboard shortcut.
    pub const fn key(self) -> char {
        match self {
            Self::Home => 'h',
            Self::Tasks => 't',
            Self::Sessions => 's',
            Self::Activity => 'a',
            Self::Settings => ',',
        }
    }
    /// Finds a place by its key. The former destinations' keys that no place or list took
    /// over still open their new homes: `n` (Now) Home, `p` and `r` (Plans, Review) Tasks.
    pub fn from_key(key: char) -> Option<Area> {
        match key {
            'n' => Some(Self::Home),
            'p' | 'r' => Some(Self::Tasks),
            _ => Self::ALL.into_iter().find(|item| item.key() == key),
        }
    }
    /// The page whose count the place's row shows: Home's needs-you count.
    pub const fn counted(self) -> Option<Destination> {
        match self {
            Self::Home => Some(Destination::Home),
            _ => None,
        }
    }
}

/// Every key that opens a place or one of Activity's pages: the places' own, the former
/// destinations' (`n p r`), and Usage's and Artifacts' (`u f`), which they keep.
pub const PLACE_KEYS: [&str; 10] = ["h", "t", "s", "a", ",", "n", "p", "r", "u", "f"];

/// A tab of the task page. The page is the one place tabs stay: the diff needs the width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TaskTab {
    #[default]
    Overview,
    Changes,
    History,
}

impl TaskTab {
    pub const ALL: [TaskTab; 3] = [Self::Overview, Self::Changes, Self::History];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Changes => "Changes",
            Self::History => "History",
        }
    }
    /// The identifier slug (`tab-task-<slug>`) and the address segment.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Changes => "changes",
            Self::History => "history",
        }
    }
    pub fn from_slug(slug: &str) -> Option<TaskTab> {
        Self::ALL.into_iter().find(|tab| tab.slug() == slug)
    }
}

/// A section of settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsSection {
    /// Device settings.
    Devices,
    /// This daemon: its host, listener and workspace.
    Daemon,
    /// Connection settings.
    Connection,
    /// Notification settings.
    Notifications,
    /// The `corral.toml` editor.
    Config,
    /// General settings.
    General,
}
impl SettingsSection {
    /// Settings sections in navigation order.
    pub const ALL: [SettingsSection; 6] = [
        Self::Devices,
        Self::Daemon,
        Self::Connection,
        Self::Notifications,
        Self::Config,
        Self::General,
    ];
    /// The display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Devices => "Devices",
            Self::Daemon => "This daemon",
            Self::Connection => "Connection",
            Self::Notifications => "Notifications",
            Self::Config => "Configuration",
            Self::General => "General",
        }
    }
    /// The storage and identifier slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Devices => "devices",
            Self::Daemon => "daemon",
            Self::Connection => "connection",
            Self::Notifications => "notifications",
            Self::Config => "config",
            Self::General => "general",
        }
    }
    /// Finds a section by its slug.
    pub fn from_slug(slug: &str) -> Option<SettingsSection> {
        Self::ALL.into_iter().find(|item| item.slug() == slug)
    }
}

/// A phone navigation tab: one per place. Settings opens from the gear in the head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhoneTab {
    Home,
    Tasks,
    Sessions,
    /// The list of Activity's pages.
    Activity,
}
impl PhoneTab {
    /// Phone tabs in display order.
    pub const ALL: [PhoneTab; 4] = [Self::Home, Self::Tasks, Self::Sessions, Self::Activity];
    /// The place the tab shows.
    pub const fn area(self) -> Area {
        match self {
            Self::Home => Area::Home,
            Self::Tasks => Area::Tasks,
            Self::Sessions => Area::Sessions,
            Self::Activity => Area::Activity,
        }
    }
    /// The display label.
    pub const fn label(self) -> &'static str {
        self.area().label()
    }
    /// The identifier slug.
    pub const fn slug(self) -> &'static str {
        self.area().slug()
    }
    /// The tab's destination, if its root is one: Activity's root lists its pages.
    pub const fn destination(self) -> Option<Destination> {
        match self {
            Self::Home => Some(Destination::Home),
            Self::Tasks => Some(Destination::Tasks),
            Self::Sessions => Some(Destination::Sessions),
            Self::Activity => None,
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
        Destination::Home
    }
}

/// Navigation state shared across presentation forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavState {
    /// The current destination.
    pub destination: Destination,
    /// The Activity page last shown, which the Activity place opens again.
    pub activity: Destination,
    /// The current settings section.
    pub settings: SettingsSection,
    /// Counts explicit section-opening intents, including a repeated opening of the same section.
    pub section_opens: u32,
    /// Grows by one (wrapping) on every `go`, `go_settings` included, so a shell can tell a
    /// navigation to the destination it already shows from no change.
    pub goes: u32,
    /// Open overlays in oldest-first order.
    pub overlays: Vec<Overlay>,
    /// Open pages in oldest-first order: a task's page or a session's, on every form.
    pub pages: Vec<Route>,
    /// The tab the task page on top shows.
    pub task_tab: TaskTab,
    /// The live record selected in each domain.
    pub selection: Selection,
    /// The record a former list-or-detail screen was asked to show in place. Nothing sets it
    /// since every record opens its page; it goes with the screens that read it.
    pub opened: Option<Live>,
    /// Grew by one on every such open.
    pub opens: u32,
}
impl NavState {
    /// Creates navigation state at the form's home destination.
    pub fn new(form: Form) -> NavState {
        NavState {
            destination: form.home(),
            activity: Destination::Feed,
            settings: SettingsSection::Devices,
            section_opens: 0,
            goes: 0,
            overlays: Vec::new(),
            pages: Vec::new(),
            task_tab: TaskTab::default(),
            selection: Selection::default(),
            opened: None,
            opens: 0,
        }
    }
    /// Navigates to a destination and closes the palette.
    pub fn go(&mut self, destination: Destination) {
        self.goes = self.goes.wrapping_add(1);
        self.destination = destination;
        if destination.area() == Area::Activity {
            self.activity = destination;
        }
        self.pages.clear();
        self.opened = None;
        self.close(Overlay::Palette);
    }
    /// The page a place opens: its own, or for Activity the page last shown there.
    pub const fn page_of(&self, area: Area) -> Destination {
        match area {
            Area::Home => Destination::Home,
            Area::Tasks => Destination::Tasks,
            Area::Sessions => Destination::Sessions,
            Area::Activity => self.activity,
            Area::Settings => Destination::Settings,
        }
    }
    /// The page a place key opens ([`PLACE_KEYS`]), from here.
    pub fn key_page(&self, key: char) -> Option<Destination> {
        match key {
            'u' => Some(Destination::Usage),
            'f' => Some(Destination::Artifacts),
            _ => Area::from_key(key).map(|area| self.page_of(area)),
        }
    }
    /// Navigates to a settings section and closes the palette.
    pub fn go_settings(&mut self, section: SettingsSection) {
        self.settings = section;
        self.section_opens = self.section_opens.wrapping_add(1);
        self.go(Destination::Settings);
    }
    /// Selects a live record and pushes its page: a session's, or the task's for a task, a
    /// plan (a task with children) and a review candidate (a task in Review) alike.
    pub fn open_live(&mut self, live: Live) {
        self.selection.set(&live);
        let page = live.page();
        self.selection.set(&page);
        if matches!(page, Live::Task(_)) && self.live() != Some(&page) {
            self.task_tab = TaskTab::default();
        }
        self.open_route(Route::Live(page));
    }
    /// Forgets the explicit open, as the way back from a narrow detail to its list did.
    pub fn dismiss_opened(&mut self) {
        self.opened = None;
    }
    fn open_route(&mut self, route: Route) {
        self.destination = route.owner();
        self.close(Overlay::Palette);
        if self.pages.last() != Some(&route) {
            self.pages.push(route);
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
            None => None,
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
    /// Handles a place or Ask key unless an overlay is open.
    pub fn press_key(&mut self, key: char) -> bool {
        if !self.overlays.is_empty() {
            return false;
        }
        if key == '/' {
            self.open(Overlay::Ask);
            true
        } else if let Some(destination) = self.key_page(key) {
            self.go(destination);
            true
        } else {
            false
        }
    }
}

/// Whether the workspace has the page. An assistant-only workspace has no board: it keeps
/// Home (its landing), Sessions, Activity › Usage and Settings, and the rest is left out
/// rather than shown unavailable.
pub fn offers(feed: &crate::model::daemon::Feed, destination: Destination) -> bool {
    use crate::model::projects::WorkspaceMode;
    WorkspaceMode::of(feed) != WorkspaceMode::AssistantOnly
        || matches!(
            destination,
            Destination::Home | Destination::Sessions | Destination::Usage | Destination::Settings
        )
}

/// The places the sidebar and the phone's tab bar list, in order.
pub fn places(feed: &crate::model::daemon::Feed) -> Vec<Area> {
    Area::PLACES
        .into_iter()
        .filter(|area| *area == Area::Activity || offers(feed, page_at_rest(*area)))
        .collect()
}

/// Activity's pages in this workspace, in order.
pub fn activity_pages(feed: &crate::model::daemon::Feed) -> Vec<Destination> {
    Destination::ACTIVITY
        .into_iter()
        .filter(|page| offers(feed, *page))
        .collect()
}

/// Every page this workspace has, in navigation order: what the palette lists.
pub fn destinations(feed: &crate::model::daemon::Feed) -> Vec<Destination> {
    Destination::ALL
        .into_iter()
        .filter(|page| offers(feed, *page))
        .collect()
}

/// The phone's tabs in this workspace.
pub fn phone_tabs(feed: &crate::model::daemon::Feed) -> Vec<PhoneTab> {
    let places = places(feed);
    PhoneTab::ALL
        .into_iter()
        .filter(|tab| places.contains(&tab.area()))
        .collect()
}

const fn page_at_rest(area: Area) -> Destination {
    match area {
        Area::Home => Destination::Home,
        Area::Tasks => Destination::Tasks,
        Area::Sessions => Destination::Sessions,
        Area::Activity => Destination::Feed,
        Area::Settings => Destination::Settings,
    }
}

#[cfg(test)]
#[path = "nav_tests.rs"]
mod tests;
