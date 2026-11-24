//! The app's address: the place the navigation shows, written as path segments (`task/T-12`,
//! `activity/usage`) so a reload, a link or the browser's Back can name it again. Where the
//! segments sit in a URL is the platform's business (`state::address`). The paths of the nine
//! former destinations and of the Project page still parse, each to its new home.

use crate::model::nav::{Area, TaskTab};
use crate::model::{Destination, Form, Id, Live, NavState, Route, SettingsSection};

/// The first segment of a task's page, a session's page and Activity's pages.
const TASK: &str = "task";
const SESSION: &str = "session";
const ACTIVITY: &str = "activity";
/// The last segment of Activity › System's Runtime view.
const RUNTIME: &str = "runtime";
/// The first segment of the former Project page.
const PROJECTS: &str = "projects";

/// What an address names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A destination's own screen.
    Destination(Destination),
    /// Activity › System showing its Runtime view.
    SystemRuntime,
    /// A Settings section.
    Settings(SettingsSection),
    /// A task's page at one of its tabs.
    Task(Id, TaskTab),
    /// A session's page.
    Session(Id),
    /// Home scoped to a project: where the former Project page went. Only an old address
    /// names it; the scope is not part of the address the app writes.
    Project(Id),
}

/// What a shell shows that the navigation itself does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    /// The Settings section the phone's stack has open over its Settings list.
    pub phone_section: Option<SettingsSection>,
    /// Activity › System shows its Runtime view.
    pub system_runtime: bool,
}

impl Place {
    /// Where the navigation is: the page on top, else the destination.
    pub fn of(nav: &NavState, form: Form, shown: Shown) -> Place {
        match nav.pages.last() {
            Some(Route::Live(Live::Session(id))) => return Place::Session(id.clone()),
            Some(Route::Live(Live::Task(id) | Live::Plan(id) | Live::Review(id))) => {
                return Place::Task(id.clone(), nav.task_tab);
            }
            None => {}
        }
        match nav.destination {
            // The phone lists the sections first; which one is open is its shell's.
            Destination::Settings if form == Form::Phone => shown
                .phone_section
                .map_or(Place::Destination(Destination::Settings), Place::Settings),
            Destination::Settings => Place::Settings(nav.settings),
            Destination::System if shown.system_runtime => Place::SystemRuntime,
            destination => Place::Destination(destination),
        }
    }

    /// The place as path segments, without a leading slash.
    pub fn path(&self) -> String {
        match self {
            Place::Destination(destination) if destination.area() == Area::Activity => {
                format!("{ACTIVITY}/{}", destination.slug())
            }
            Place::Destination(destination) => destination.slug().to_owned(),
            Place::SystemRuntime => {
                format!("{ACTIVITY}/{}/{RUNTIME}", Destination::System.slug())
            }
            Place::Settings(section) => {
                format!("{}/{}", Destination::Settings.slug(), section.slug())
            }
            Place::Task(id, TaskTab::Overview) => format!("{TASK}/{}", escape(id.as_str())),
            Place::Task(id, tab) => format!("{TASK}/{}/{}", escape(id.as_str()), tab.slug()),
            Place::Session(id) => format!("{SESSION}/{}", escape(id.as_str())),
            Place::Project(id) => format!("{PROJECTS}/{}", escape(id.as_str())),
        }
    }

    /// The place a path names; none for the site's root or a path that names nothing here.
    pub fn parse(path: &str) -> Option<Place> {
        let path = path.trim_matches('/');
        let (first, rest) = path.split_once('/').unwrap_or((path, ""));
        let id = |rest: &str| unescape(rest).filter(|id| !id.is_empty()).map(Id::from);
        match first {
            TASK => {
                // A task's id has no slash, so a last segment naming a tab is the tab.
                let (name, tab) = rest
                    .rsplit_once('/')
                    .and_then(|(name, tab)| Some((name, TaskTab::from_slug(tab)?)))
                    .unwrap_or((rest, TaskTab::Overview));
                return id(name).map(|id| Place::Task(id, tab));
            }
            SESSION => return id(rest).map(Place::Session),
            PROJECTS => return id(rest).map(Place::Project),
            ACTIVITY => {
                if rest.split_once('/') == Some((Destination::System.slug(), RUNTIME)) {
                    return Some(Place::SystemRuntime);
                }
                let page = Destination::from_slug(rest)
                    .filter(|page| page.area() == Area::Activity)
                    .unwrap_or(Destination::Feed);
                return Some(Place::Destination(page));
            }
            _ => {}
        }
        let destination = Destination::from_slug(first)?;
        // The former destinations' record paths: `now/T-1`, `plans/T-1`, `review/T-1`,
        // `agents/B-1`.
        let record = match (destination, first) {
            (Destination::Settings, _) => SettingsSection::from_slug(rest).map(Place::Settings),
            (Destination::Home, "now") | (Destination::Tasks, "plans" | "review") => {
                id(rest).map(|id| Place::Task(id, TaskTab::Overview))
            }
            (Destination::Sessions, "agents") => id(rest).map(Place::Session),
            _ => None,
        };
        Some(record.unwrap_or(Place::Destination(destination)))
    }

    /// Goes to the place: its page alone over its destination, whatever was open before. A
    /// project's address goes Home; setting the scope is the caller's.
    pub fn open(self, nav: &mut NavState) {
        match self {
            Place::Destination(destination) => nav.go(destination),
            // The view itself is the System page's state, which the caller sets.
            Place::SystemRuntime => nav.go(Destination::System),
            Place::Settings(section) => nav.go_settings(section),
            Place::Task(id, tab) => {
                nav.pages.clear();
                nav.open_live(Live::Task(id));
                nav.task_tab = tab;
            }
            Place::Session(id) => {
                nav.pages.clear();
                nav.open_live(Live::Session(id));
            }
            Place::Project(_) => nav.go(Destination::Home),
        }
    }

    /// Whether moving between the two only changes the tab of one task's page: not a step the
    /// browser's Back returns to.
    pub fn same_page(&self, other: &Place) -> bool {
        matches!((self, other), (Place::Task(a, _), Place::Task(b, _)) if a == b)
    }
}

/// The path of an absolute URL, without its query and fragment.
pub fn url_path(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = rest.find('/').map_or("", |at| &rest[at..]);
    path.split(['?', '#']).next().unwrap_or(path)
}

/// An id as path segments: its own slashes stay (a child's `B-1/child/c2` reads as it is),
/// everything else outside letters, digits, `-`, `_` and `~` is percent-encoded. A dot is too:
/// a web host answers a path with the page only when its last segment has no file extension.
fn escape(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for byte in id.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'~' | b'/' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The id [`escape`] wrote; none when the escapes are not UTF-8 text.
fn unescape(path: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(path.len());
    let mut rest = path.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        rest = after;
        if byte != b'%' {
            bytes.push(byte);
            continue;
        }
        let (hex, after) = rest.split_at_checked(2)?;
        bytes.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
        rest = after;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOWN: Shown = Shown {
        phone_section: None,
        system_runtime: false,
    };

    /// The Runtime view had no address of its own: a reload or a link must open it again,
    /// and only on the System page.
    #[test]
    fn the_runtime_view_has_its_own_address() {
        let runtime = Shown {
            phone_section: None,
            system_runtime: true,
        };
        let mut nav = NavState::new(Form::Desktop);
        nav.go(Destination::System);
        let place = Place::of(&nav, Form::Desktop, runtime);
        assert_eq!(place.path(), "activity/system/runtime");
        assert_eq!(Place::parse(&place.path()), Some(Place::SystemRuntime));
        nav.go(Destination::Usage);
        assert_eq!(
            Place::of(&nav, Form::Desktop, runtime),
            Place::Destination(Destination::Usage)
        );
    }

    fn reloaded(nav: &NavState, form: Form) -> Place {
        let place = Place::of(nav, form, SHOWN);
        let mut fresh = NavState::new(form);
        Place::parse(&place.path())
            .unwrap_or_else(|| panic!("{} names nothing", place.path()))
            .open(&mut fresh);
        assert_eq!(fresh.pages, nav.pages, "{}", place.path());
        assert_eq!(fresh.destination, nav.destination, "{}", place.path());
        Place::of(&fresh, form, SHOWN)
    }

    /// A reload lost everything but the destination: each place a form can show has to come
    /// back from its own path, its page and the task page's tab included.
    #[test]
    fn a_reload_lands_on_the_place_the_path_names() {
        for form in [Form::Desktop, Form::Phone] {
            let mut nav = NavState::new(form);
            for destination in Destination::ALL {
                nav.go(destination);
                assert_eq!(reloaded(&nav, form), Place::of(&nav, form, SHOWN));
            }
            for live in [
                Live::Task(Id::from("T-12")),
                Live::Session(Id::from("B-k3x9/child/review.1")),
                Live::Session(Id::from("T-871_R1")),
            ] {
                nav.go(Destination::Usage);
                nav.open_live(live);
                assert_eq!(reloaded(&nav, form), Place::of(&nav, form, SHOWN));
            }
            nav.go(Destination::Usage);
            nav.open_live(Live::Task(Id::from("T-12")));
            nav.task_tab = TaskTab::Changes;
            assert_eq!(
                reloaded(&nav, form),
                Place::Task(Id::from("T-12"), TaskTab::Changes)
            );
        }
        let mut nav = NavState::new(Form::Desktop);
        nav.go_settings(SettingsSection::Config);
        assert_eq!(
            reloaded(&nav, Form::Desktop),
            Place::Settings(SettingsSection::Config)
        );
    }

    /// The phone's open section was only "Settings" in the address, so a reload there showed
    /// the Settings list: the section its stack shows is the place, and its path asks the
    /// shell to open that section again.
    #[test]
    fn a_phone_reload_inside_a_settings_section_opens_it_again() {
        let mut nav = NavState::new(Form::Phone);
        nav.go_settings(SettingsSection::Config);
        let list = Place::of(&nav, Form::Phone, SHOWN);
        assert_eq!(list, Place::Destination(Destination::Settings));
        let open = Shown {
            phone_section: Some(SettingsSection::Config),
            system_runtime: false,
        };
        let section = Place::of(&nav, Form::Phone, open);
        assert_ne!(section.path(), list.path());
        let mut fresh = NavState::new(Form::Phone);
        Place::parse(&section.path())
            .unwrap_or_else(|| panic!("{} names nothing", section.path()))
            .open(&mut fresh);
        assert_eq!(
            (fresh.settings, fresh.section_opens),
            (SettingsSection::Config, 1)
        );
    }

    /// The host serves the page only for a last segment without an extension, and a path that
    /// names nothing must leave the stored destination alone rather than land somewhere.
    #[test]
    fn paths_keep_no_dot_and_unknown_ones_name_nothing() {
        let child = Place::Session(Id::from("B-1/child/gpt-5.2"));
        assert_eq!(child.path(), "session/B-1/child/gpt-5%2E2");
        for path in [
            "",
            "/",
            "nowhere",
            "projects",
            "task",
            "task/%zz",
            "session/%FF",
        ] {
            assert_eq!(Place::parse(path), None, "{path}");
        }
        assert_eq!(url_path("https://corral.test:8790"), "");
        assert_eq!(
            Place::parse(url_path("https://corral.test:8790/task/T-7/history?x=1#y")),
            Some(Place::Task(Id::from("T-7"), TaskTab::History))
        );
    }

    /// A bookmark or a link from before the four places named a destination, a record under
    /// one or a project's page: each has to open its new home, never the root.
    #[test]
    fn the_former_addresses_open_their_new_homes() {
        let task = |id: &str| Some(Place::Task(Id::from(id), TaskTab::Overview));
        let page = |destination| Some(Place::Destination(destination));
        for (path, place) in [
            ("now", page(Destination::Home)),
            ("inbox", page(Destination::Home)),
            ("now/T-1", task("T-1")),
            ("plans", page(Destination::Tasks)),
            ("plans/T-885", task("T-885")),
            ("review/T-12", task("T-12")),
            ("agents", page(Destination::Sessions)),
            ("/builds/", page(Destination::Sessions)),
            (
                "agents/B-1/child/c2",
                Some(Place::Session(Id::from("B-1/child/c2"))),
            ),
            ("changes", page(Destination::Feed)),
            ("timeline", page(Destination::Timeline)),
            ("usage", page(Destination::Usage)),
            ("artifacts", page(Destination::Artifacts)),
            ("activity", page(Destination::Feed)),
            (
                "projects/my%20site",
                Some(Place::Project(Id::from("my site"))),
            ),
        ] {
            assert_eq!(Place::parse(path), place, "{path}");
        }
        let mut nav = NavState::new(Form::Desktop);
        nav.go(Destination::Usage);
        Place::Project(Id::from("site")).open(&mut nav);
        assert_eq!(nav.destination, Destination::Home);
    }

    /// Switching a task page's tab must not fill the browser's history: only a new page is a
    /// step back.
    #[test]
    fn a_tab_of_the_same_task_is_the_same_page() {
        let task = |id: &str, tab| Place::Task(Id::from(id), tab);
        assert!(task("T-1", TaskTab::Overview).same_page(&task("T-1", TaskTab::Changes)));
        assert!(!task("T-1", TaskTab::Overview).same_page(&task("T-2", TaskTab::Overview)));
        assert!(!Place::Destination(Destination::Tasks).same_page(&task("T-1", TaskTab::Overview)));
        let session = |id: &str| Place::Session(Id::from(id));
        assert!(!session("A-1").same_page(&session("A-2")));
    }
}
