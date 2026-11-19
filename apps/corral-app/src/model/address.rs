//! The app's address: the place the navigation shows, written as path segments (`agents/B-12`)
//! so a reload, a link or the browser's Back can name it again. Where the segments sit in a
//! URL is the platform's business (`state::address`).

use crate::model::{Destination, Form, Id, Live, NavState, Route, SettingsSection};

/// The first segment of a project page; no destination has this slug.
const PROJECTS: &str = "projects";

/// What an address names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A destination's own screen.
    Destination(Destination),
    /// A Settings section.
    Settings(SettingsSection),
    /// A record: a session's page, or a task, plan or review shown in its destination.
    Live(Live),
    /// A project's page.
    Project(Id),
}

/// What a shell shows that the navigation itself does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    /// Whether Now shows its selected task beside the list.
    pub inspector: bool,
    /// The Settings section the phone's stack has open over its Settings list.
    pub phone_section: Option<SettingsSection>,
}

impl Place {
    /// Where the navigation is. Off the phone a task, plan or review is selected beside its
    /// list rather than pushed, so the selection is part of the place there.
    pub fn of(nav: &NavState, form: Form, shown: Shown) -> Place {
        match nav.pages.last() {
            Some(Route::Live(live)) => return Place::Live(live.clone()),
            Some(Route::Project(id)) => return Place::Project(id.clone()),
            None => {}
        }
        let selected = |id: &Option<Id>, live: fn(Id) -> Live| {
            id.clone()
                .filter(|_| form != Form::Phone)
                .map_or(Place::Destination(nav.destination), |id| {
                    Place::Live(live(id))
                })
        };
        match nav.destination {
            // The phone lists the sections first; which one is open is its shell's.
            Destination::Settings if form == Form::Phone => shown
                .phone_section
                .map_or(Place::Destination(Destination::Settings), Place::Settings),
            Destination::Settings => Place::Settings(nav.settings),
            Destination::Now if shown.inspector => selected(&nav.selection.task, Live::Task),
            Destination::Plans => selected(&nav.selection.plan, Live::Plan),
            Destination::Review => selected(&nav.selection.review, Live::Review),
            destination => Place::Destination(destination),
        }
    }

    /// The place as path segments, without a leading slash.
    pub fn path(&self) -> String {
        match self {
            Place::Destination(destination) => destination.slug().to_owned(),
            Place::Settings(section) => {
                format!("{}/{}", Destination::Settings.slug(), section.slug())
            }
            Place::Live(live) => format!("{}/{}", live.owner().slug(), escape(live.id().as_str())),
            Place::Project(id) => format!("{PROJECTS}/{}", escape(id.as_str())),
        }
    }

    /// The place a path names; none for the site's root or a path that names nothing here.
    pub fn parse(path: &str) -> Option<Place> {
        let path = path.trim_matches('/');
        let (first, rest) = path.split_once('/').unwrap_or((path, ""));
        let id = || unescape(rest).filter(|id| !id.is_empty()).map(Id::from);
        if first == PROJECTS {
            return id().map(Place::Project);
        }
        let destination = Destination::from_slug(first)?;
        let record = match destination {
            Destination::Settings => SettingsSection::from_slug(rest).map(Place::Settings),
            Destination::Now => id().map(|id| Place::Live(Live::Task(id))),
            Destination::Plans => id().map(|id| Place::Live(Live::Plan(id))),
            Destination::Review => id().map(|id| Place::Live(Live::Review(id))),
            Destination::Agents => id().map(|id| Place::Live(Live::Session(id))),
            _ => None,
        };
        Some(record.unwrap_or(Place::Destination(destination)))
    }

    /// Goes to the place: its page alone over its destination, whatever was open before.
    pub fn open(self, nav: &mut NavState, form: Form) {
        match self {
            Place::Destination(destination) => nav.go(destination),
            Place::Settings(section) => nav.go_settings(section),
            Place::Live(live) => {
                nav.pages.clear();
                nav.open_live(live, form);
            }
            Place::Project(id) => {
                nav.pages.clear();
                nav.open_project(id);
            }
        }
    }

    /// Whether moving between the two only changes the record selected beside a list: not a
    /// step the browser's Back returns to.
    pub fn same_page(&self, other: &Place) -> bool {
        self.list().is_some() && self.list() == other.list()
    }

    fn list(&self) -> Option<Destination> {
        match self {
            Place::Destination(destination) => Some(*destination),
            Place::Live(live) if !matches!(live, Live::Session(_)) => Some(live.owner()),
            Place::Live(_) | Place::Settings(_) | Place::Project(_) => None,
        }
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
        inspector: true,
        phone_section: None,
    };

    fn reloaded(nav: &NavState, form: Form) -> Place {
        let place = Place::of(nav, form, SHOWN);
        let mut fresh = NavState::new(form);
        Place::parse(&place.path())
            .unwrap_or_else(|| panic!("{} names nothing", place.path()))
            .open(&mut fresh, form);
        assert_eq!(fresh.pages, nav.pages, "{}", place.path());
        assert_eq!(fresh.destination, nav.destination, "{}", place.path());
        Place::of(&fresh, form, SHOWN)
    }

    /// A reload lost everything but the destination: each place a form can show has to come
    /// back from its own path, pages and selection included.
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
                Live::Plan(Id::from("T-885")),
                Live::Review(Id::from("T-12")),
                Live::Session(Id::from("B-k3x9/child/review.1")),
                Live::Session(Id::from("T-871_R1")),
            ] {
                nav.go(Destination::Usage);
                nav.open_live(live.clone(), form);
                assert_eq!(reloaded(&nav, form), Place::Live(live));
            }
            nav.go(Destination::Usage);
            nav.open_project(Id::from("my site"));
            assert_eq!(reloaded(&nav, form), Place::Project(Id::from("my site")));
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
            ..SHOWN
        };
        let section = Place::of(&nav, Form::Phone, open);
        assert_ne!(section.path(), list.path());
        let mut fresh = NavState::new(Form::Phone);
        Place::parse(&section.path())
            .unwrap_or_else(|| panic!("{} names nothing", section.path()))
            .open(&mut fresh, Form::Phone);
        assert_eq!(
            (fresh.settings, fresh.section_opens),
            (SettingsSection::Config, 1)
        );
    }

    /// The host serves the page only for a last segment without an extension, and a path that
    /// names nothing must leave the stored destination alone rather than land somewhere.
    #[test]
    fn paths_keep_no_dot_and_unknown_ones_name_nothing() {
        let child = Place::Live(Live::Session(Id::from("B-1/child/gpt-5.2")));
        assert_eq!(child.path(), "agents/B-1/child/gpt-5%2E2");
        for path in ["", "/", "nowhere", "projects", "now/%zz", "now/%FF"] {
            let place = Place::parse(path);
            assert!(
                matches!(place, None | Some(Place::Destination(Destination::Now))),
                "{path}: {place:?}"
            );
        }
        assert_eq!(Place::parse(""), None);
        assert_eq!(Place::parse("nowhere"), None);
        assert_eq!(url_path("https://corral.test:8790"), "");
        assert_eq!(
            Place::parse(url_path("https://corral.test:8790/plans/T-7?x=1#y")),
            Some(Place::Live(Live::Plan(Id::from("T-7"))))
        );
        assert_eq!(
            Place::parse("/builds/"),
            Some(Place::Destination(Destination::Agents))
        );
    }

    /// Walking a list must not fill the browser's history: only a new page is a step back.
    #[test]
    fn selecting_beside_a_list_is_the_same_page() {
        let now = Place::Destination(Destination::Now);
        let task = |id: &str| Place::Live(Live::Task(Id::from(id)));
        assert!(now.same_page(&task("T-1")));
        assert!(task("T-1").same_page(&task("T-2")));
        assert!(!task("T-1").same_page(&Place::Live(Live::Plan(Id::from("T-1")))));
        let session = |id: &str| Place::Live(Live::Session(Id::from(id)));
        assert!(!session("A-1").same_page(&session("A-2")));
        assert!(!session("A-1").same_page(&Place::Destination(Destination::Agents)));
    }
}
