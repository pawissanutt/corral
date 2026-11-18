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
    /// The current workspace; its first group is what needs you.
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
    pub const ALL: [Destination; 8] = [
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
    /// Finds a destination by its slug; the merged Inbox and Builds slugs open their new homes.
    pub fn from_slug(slug: &str) -> Option<Destination> {
        match slug {
            "inbox" => Some(Self::Now),
            "builds" => Some(Self::Agents),
            _ => Self::ALL.into_iter().find(|item| item.slug() == slug),
        }
    }
    /// The destination's keyboard shortcut.
    pub const fn key(self) -> char {
        match self {
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
    /// Finds a destination by its keyboard shortcut; `i`, the former Inbox key, opens Now.
    pub fn from_key(key: char) -> Option<Destination> {
        if key == 'i' {
            return Some(Self::Now);
        }
        Self::ALL.into_iter().find(|item| item.key() == key)
    }
    /// The phone tab containing this destination.
    pub const fn phone_tab(self) -> PhoneTab {
        match self {
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

/// A phone navigation tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhoneTab {
    /// The current workspace tab, opening at what needs you.
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
    pub const ALL: [PhoneTab; 4] = [Self::Now, Self::Agents, Self::Plans, Self::More];
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
            Self::Now => "Now",
            Self::Agents => "Agents",
            Self::Plans => "Plans",
            Self::More => "More",
        }
    }
    /// The identifier slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Now => "now",
            Self::Agents => "agents",
            Self::Plans => "plans",
            Self::More => "more",
        }
    }
    /// The tab's zero-based index.
    pub const fn index(self) -> u32 {
        match self {
            Self::Now => 0,
            Self::Agents => 1,
            Self::Plans => 2,
            Self::More => 3,
        }
    }
    /// Finds a tab by its zero-based index.
    pub fn from_index(index: u32) -> Option<PhoneTab> {
        Self::ALL.into_iter().find(|item| item.index() == index)
    }
    /// The tab's destination, if it opens one directly.
    pub const fn destination(self) -> Option<Destination> {
        match self {
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
        Destination::Now
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
    /// The record the last navigation opened by name (a link, a row elsewhere, the palette)
    /// and that selects in place rather than pushing a page; none after a plain `go` or once
    /// dismissed. A narrow list-or-detail layout shows the detail for it.
    pub opened: Option<Live>,
    /// Grows by one (wrapping) on every such open, so a screen already showing the
    /// destination can tell a repeated open of the same record from no change.
    pub opens: u32,
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
            opened: None,
            opens: 0,
        }
    }
    /// Navigates to a destination and closes the palette.
    pub fn go(&mut self, destination: Destination) {
        self.goes = self.goes.wrapping_add(1);
        self.destination = destination;
        self.pages.clear();
        self.opened = None;
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
        if push {
            self.opened = None;
        } else {
            self.opened = Some(live.clone());
            self.opens = self.opens.wrapping_add(1);
        }
        self.open_route(Route::Live(live), push);
    }
    /// Forgets the explicit open, as the way back from a narrow detail to its list does.
    pub fn dismiss_opened(&mut self) {
        self.opened = None;
    }
    /// Projects are local pages on every form; no daemon detail subscription is needed.
    pub fn open_project(&mut self, id: crate::model::Id) {
        self.open_route(Route::Project(id), true);
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

/// Explicit record navigation changes only a destination scope that would hide the record.
/// Legacy plan rows have unknown project membership and already ignore project filters.
pub fn explicit_record_filter(
    feed: &crate::model::daemon::Feed,
    filters: &crate::model::projects::Filters,
    live: &Live,
) -> Option<(
    crate::model::projects::FilterScreen,
    crate::model::projects::ProjectFilter,
)> {
    use crate::model::projects::{FilterScreen, ProjectFilter};

    let store = feed.store.as_deref()?;
    let screen = FilterScreen::of(live.owner())?;
    let filter = filters.get(store, screen, feed);
    if filter == ProjectFilter::All {
        return None;
    }
    let projects: &[String] = match live {
        Live::Task(id) | Live::Review(id) | Live::Plan(id) => {
            feed.board.task(id)?.row.task.project.as_slice()
        }
        _ => return None,
    };
    let visible = match &filter {
        ProjectFilter::All => true,
        ProjectFilter::Project(project) => projects.iter().any(|name| name == project.as_str()),
        ProjectFilter::Unassigned => projects.is_empty(),
    };
    if visible {
        return None;
    }
    let scope = match projects {
        [] => ProjectFilter::Unassigned,
        [project] => ProjectFilter::Project(crate::model::Id::from(project.as_str())),
        _ => ProjectFilter::All,
    };
    Some((screen, scope))
}

/// Navigation targets keep Ask a chat interaction rather than a board destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationTarget {
    Ask,
    Destination(Destination),
    Tab(PhoneTab),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationItem {
    pub id: String,
    pub label: &'static str,
    pub target: NavigationTarget,
}

/// Board destinations disappear in assistant workspaces, rather than becoming disabled.
pub fn navigation(feed: &crate::model::daemon::Feed) -> Vec<NavigationItem> {
    use crate::model::projects::WorkspaceMode;
    let mut items = vec![NavigationItem {
        id: "nav-ask".into(),
        label: "Ask",
        target: NavigationTarget::Ask,
    }];
    let assistant = WorkspaceMode::of(feed) == WorkspaceMode::AssistantOnly;
    items.extend(
        Destination::ALL
            .into_iter()
            .filter(|destination| {
                !assistant
                    || matches!(
                        destination,
                        Destination::Agents | Destination::Usage | Destination::Settings
                    )
            })
            .map(|destination| NavigationItem {
                id: format!("nav-{}", destination.slug()),
                label: destination.label(),
                target: NavigationTarget::Destination(destination),
            }),
    );
    items
}

pub fn phone_navigation(feed: &crate::model::daemon::Feed) -> Vec<NavigationItem> {
    if crate::model::projects::WorkspaceMode::of(feed)
        == crate::model::projects::WorkspaceMode::AssistantOnly
    {
        return navigation(feed)
            .into_iter()
            .map(|mut item| {
                item.id = item.id.replacen("nav-", "tab-", 1);
                item
            })
            .collect();
    }
    PhoneTab::ALL
        .into_iter()
        .map(|tab| NavigationItem {
            id: format!("tab-{}", tab.slug()),
            label: tab.label(),
            target: NavigationTarget::Tab(tab),
        })
        .collect()
}

#[cfg(test)]
#[path = "nav_tests.rs"]
mod tests;
