//! What the app says and offers while it has no connection to corrald, on every form: why a
//! destination is empty and the way to Settings › Connection; Connection's own guide (a native
//! app starts with the daemon's socket and enrols; a browser pairs with a code from a trusted
//! device) with its commands and diagnostics; and this device's local preferences, which need
//! no connection.

use crate::model::daemon::Feed;
use crate::model::devices::{DeviceLink, PairBrowser, PairClient, pair_browser};
use crate::model::health::DataState;
use crate::model::hue::Hue;
use crate::model::nav::{SidebarPref, ThemeChoice};

/// Why a destination shows nothing while disconnected, and the two ways on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub reason: String,
    /// Opens Settings › Connection.
    pub connect: &'static str,
    /// Switches to the explicit, labelled sample-data preview.
    pub preview: &'static str,
}

pub const PREVIEW: &str = "Preview with sample data";

pub fn unavailable(feed: &Feed, client: Option<PairClient>) -> Unavailable {
    let pairing = pair_browser(feed);
    let (reason, connect) = match (client, &pairing, &feed.device) {
        (Some(client), _, DeviceLink::Failed(line))
        | (Some(client), PairBrowser::Failed { line }, _) => (
            format!("This {} couldn't connect to corrald: {line}", client.noun()),
            "Open Connection",
        ),
        (Some(PairClient::Browser), _, _) => (
            "This browser isn't paired with corrald, so there is nothing to show yet. Pairing takes a code from a device you already trust.".to_owned(),
            "Pair this browser",
        ),
        (Some(PairClient::Native), _, _) => (
            "This device isn't paired with corrald, so there is nothing to show yet. Pairing takes corrald's address and a code from a device you already trust.".to_owned(),
            "Pair this device",
        ),
        (None, _, _) => (
            match &feed.problem {
                Some(problem) => format!("This app isn't connected to corrald: {problem}"),
                None => "This app isn't connected to corrald: CORRAL_SOCKET is not set, so it doesn't know where corrald listens.".to_owned(),
            },
            "Set up the connection",
        ),
    };
    Unavailable {
        reason,
        connect,
        preview: PREVIEW,
    }
}

/// How far a guide step is; the screen draws it as done, current or still to come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Done,
    Current,
    Later,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub progress: Progress,
    pub title: &'static str,
    pub detail: &'static str,
}

/// Settings › Connection while there is no connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guide {
    pub status: String,
    pub hue: Hue,
    /// The six-digit code to compare, while a pairing waits for the trusted device.
    pub sas: Option<String>,
    pub steps: Vec<Step>,
    /// Commands and configuration to copy, in mono: (what it is, the text).
    pub commands: Vec<(&'static str, String)>,
    /// Why pairing fails and what to do, for a browser.
    pub failures: Vec<(&'static str, &'static str)>,
    /// What the app knows of its connection now.
    pub details: Vec<(&'static str, String)>,
}

fn step(progress: Progress, title: &'static str, detail: &'static str) -> Step {
    Step {
        progress,
        title,
        detail,
    }
}

/// A native app without a socket: start corrald, start the app with its socket, then enrol.
pub fn native_guide(feed: &Feed, terminal: bool) -> Guide {
    let start = if terminal {
        "CORRAL_SOCKET=<workspace>/.corral-next/daemon.sock nsh run --target tui"
    } else {
        "export CORRAL_SOCKET=<workspace>/.corral-next/daemon.sock"
    };
    let mut details = vec![
        (
            "Endpoint",
            feed.endpoint
                .clone()
                .unwrap_or_else(|| "not configured · CORRAL_SOCKET is not set".into()),
        ),
        (
            "Data shown",
            "none until connected; sample data only in the preview you choose".into(),
        ),
    ];
    if let Some(problem) = &feed.problem {
        details.push(("Problem", problem.clone()));
    }
    Guide {
        status: match &feed.problem {
            Some(problem) => format!("This app isn't connected to corrald: {problem}"),
            None => "This app isn't connected to corrald. It reaches corrald through the workspace's Unix socket, named by CORRAL_SOCKET, which is not set.".into(),
        },
        hue: Hue::Held,
        sas: None,
        steps: vec![
            step(
                Progress::Later,
                "Make sure corrald runs",
                "The status command below reports its health. Without a service manager, the run command keeps corrald running in a terminal.",
            ),
            step(
                Progress::Current,
                "Start this app with the daemon's socket",
                if terminal {
                    "Quit, then start the app again with the start command below; it sets CORRAL_SOCKET to the path corrald listens on."
                } else {
                    "Quit, then start the app again from a shell where the command below set CORRAL_SOCKET to the path corrald listens on."
                },
            ),
            step(
                Progress::Later,
                "Enrol this device",
                "Settings › Connection then asks for a device name. Enrolling records this device in the workspace; you can revoke it later.",
            ),
        ],
        commands: vec![
            ("Health", "corrald-next status".into()),
            ("Run in a terminal", "corrald-next run".into()),
            ("Start the app", start.into()),
        ],
        failures: Vec::new(),
        details,
    }
}

/// A pairing client that is not paired: turn on the listener, get a pairing code, pair and
/// compare codes. `origin` is a browser page's origin, which corrald must list as a `web_origin`;
/// the Windows app names its own origin from the address its user types.
pub fn pair_guide(feed: &Feed, client: PairClient, origin: Option<&str>) -> Guide {
    let state = pair_browser(feed);
    let (status, hue, sas, progress) = pair_status(&state, client);
    let mut details = vec![(
        "Endpoint",
        feed.endpoint.clone().unwrap_or_else(|| match client {
            PairClient::Browser => "taken from the pairing code".into(),
            PairClient::Native => "the address you enter below".into(),
        }),
    )];
    if let Some(problem) = &feed.problem {
        details.push(("Problem", problem.clone()));
    }
    let (steps, commands, failures) = match client {
        PairClient::Browser => browser_help(progress, origin),
        PairClient::Native => native_help(progress),
    };
    Guide {
        status,
        hue,
        sas,
        steps,
        commands,
        failures,
        details,
    }
}

/// Where a pairing stands, in the words of the client that is pairing, and how far the three
/// guide steps are.
fn pair_status(
    state: &PairBrowser,
    client: PairClient,
) -> (String, Hue, Option<String>, [Progress; 3]) {
    use Progress::{Current, Done, Later};
    let browser = client == PairClient::Browser;
    match state {
        PairBrowser::Form { prefilled: true } if browser => (
            "The code from your pairing link is filled in. Press Pair, then check that the six-digit code here matches the one on your trusted device, and confirm there.".to_owned(),
            Hue::Neutral,
            None,
            [Done, Done, Current],
        ),
        PairBrowser::Form { .. } if browser => (
            "This browser isn't paired with corrald. Paste a pairing code from a trusted device below, then press Pair.".to_owned(),
            Hue::Neutral,
            None,
            [Later, Later, Current],
        ),
        PairBrowser::Form { .. } => (
            "This device isn't paired with corrald. Enter corrald's address and a pairing code from a trusted device below, then press Pair.".to_owned(),
            Hue::Neutral,
            None,
            [Later, Later, Current],
        ),
        PairBrowser::Connecting if browser => (
            "Connecting to corrald with the code…".to_owned(),
            Hue::Neutral,
            None,
            [Done, Done, Current],
        ),
        PairBrowser::Connecting => (
            "Connecting to corrald at the address you entered…".to_owned(),
            Hue::Neutral,
            None,
            [Done, Done, Current],
        ),
        PairBrowser::Claimed { sas } => (
            "Compare codes: confirm on your trusted device only if it shows this same code. Its confirmation window lasts two minutes.".to_owned(),
            Hue::Held,
            Some(sas.clone()),
            [Done, Done, Current],
        ),
        PairBrowser::Paired { line } => (line.clone(), Hue::Ok, None, [Done, Done, Done]),
        PairBrowser::Failed { line } => (
            format!("Pairing didn't finish: {line}"),
            Hue::Fail,
            None,
            [Later, Later, Current],
        ),
        PairBrowser::Blocked { line } => (
            format!("Pairing can't start from this page: {line}"),
            Hue::Fail,
            None,
            [Later, Later, Later],
        ),
    }
}

type Help = (
    Vec<Step>,
    Vec<(&'static str, String)>,
    Vec<(&'static str, &'static str)>,
);

fn browser_help(progress: [Progress; 3], origin: Option<&str>) -> Help {
    let origin = origin.unwrap_or("<this page's origin>");
    (
        vec![
            step(
                progress[0],
                "Turn on the browser listener",
                "It is off by default. On the daemon host, add the [daemon] table below to .corral-next/corral.toml with the address corrald should listen on and this page's origin, then restart corrald-next. Or start it with the run command below.",
            ),
            step(
                progress[1],
                "Get a pairing link",
                "Run corrald-next pair on the daemon host. It prints a link to open here, then waits for this browser. A device that is already paired can also make one: Settings › Devices › Pair a device. A link works once, for two minutes.",
            ),
            step(
                progress[2],
                "Pair and compare codes",
                "Press Pair here, and this browser shows a six-digit code. The same command on the daemon host then shows this browser's name and code and asks. Answer y only if the codes match. A device that is already paired can confirm instead.",
            ),
        ],
        vec![
            (
                ".corral-next/corral.toml",
                format!("[daemon]\nlisten = \"<address:port>\"\nweb_origin = [\"{origin}\"]"),
            ),
            (
                "Or start corrald with",
                format!("corrald-next run --listen <address:port> --web-origin {origin}"),
            ),
            (
                "Print the pairing link, wait, and ask",
                "corrald-next pair".into(),
            ),
        ],
        vec![
            (
                "Can't reach corrald",
                "The listener is off, corrald-next wasn't restarted after the change, or this page's origin isn't its web_origin.",
            ),
            (
                "Code expired or already used",
                "A code works once, for two minutes. Get a new one.",
            ),
            (
                "Codes don't match",
                "Cancel on the trusted device and start again: the code didn't come from your corrald.",
            ),
            (
                "corrald didn't prove its identity",
                "corrald was reinstalled or its key changed. Forget this browser, then pair again.",
            ),
        ],
    )
}

/// The Windows app reaches corrald in WSL2, on this PC, or on a remote Linux or macOS host.
/// It names the address itself and sends the origin that address implies, which corrald must
/// list: `http://127.0.0.1:<port>` for a loopback listener, `https://<host>` for a `wss://` one.
fn native_help(progress: [Progress; 3]) -> Help {
    (
        vec![
            step(
                progress[0],
                "Turn on the listener where corrald runs",
                "It is off by default. In WSL2, listen on 127.0.0.1 (WSL2 forwards its loopback ports to Windows) and list that address as the web_origin. A remote Linux or macOS daemon is reached at wss://<host> behind TLS, so it lists its https://<host> origin instead. Add the [daemon] table below to .corral-next/corral.toml, then restart corrald-next, or start it with the run command.",
            ),
            step(
                progress[1],
                "Get a pairing link or code",
                "Run corrald-next pair on the daemon host. It prints a pairing link, then waits for this device. Copy the whole link, or just its corral-pair-… code (after #pair=); either works here. A device that is already paired can also make one: Settings › Devices › Pair a device. A code works once, for two minutes.",
            ),
            step(
                progress[2],
                "Enter the address, pair and compare codes",
                "Type the listener's address (for example ws://127.0.0.1:7841) and paste the code or link below, then press Pair. This device shows a six-digit code, and the same command on the daemon host shows this device's name and code and asks. Answer y only if the codes match. A device that is already paired can confirm instead.",
            ),
        ],
        vec![
            (
                "WSL2 or this PC · .corral-next/corral.toml",
                "[daemon]\nlisten = \"127.0.0.1:7841\"\nweb_origin = [\"http://127.0.0.1:7841\"]".into(),
            ),
            (
                "Or start corrald in WSL2 with",
                "corrald-next run --listen 127.0.0.1:7841 --web-origin http://127.0.0.1:7841".into(),
            ),
            (
                "Remote Linux or macOS host · .corral-next/corral.toml",
                "[daemon]\nlisten = \"<address:port>\"\nweb_endpoint = \"wss://<host>\"\nweb_origin = [\"https://<host>\"]".into(),
            ),
            (
                "Print the pairing code, wait, and ask",
                "corrald-next pair".into(),
            ),
        ],
        vec![
            (
                "Can't reach corrald",
                "The listener is off, corrald-next wasn't restarted after the change, the port is wrong, or the daemon's web_origin doesn't list the origin that matches the address you typed.",
            ),
            (
                "Address doesn't match the code",
                "The code names the address it was made for. Use that one: ws://127.0.0.1:7841 for WSL2 or this PC, wss://<host> for a remote daemon.",
            ),
            (
                "Code expired or already used",
                "A code works once, for two minutes. Get a new one.",
            ),
            (
                "Codes don't match",
                "Cancel on the trusted device and start again: the code didn't come from your corrald.",
            ),
            (
                "corrald didn't prove its identity",
                "corrald was reinstalled or its key changed. Forget this device, then pair again.",
            ),
        ],
    )
}

/// Theme choices as General offers them.
pub const THEMES: [(ThemeChoice, &str); 3] = [
    (ThemeChoice::System, "System"),
    (ThemeChoice::Light, "Light"),
    (ThemeChoice::Dark, "Dark"),
];

/// The wide sidebar choices; a narrow window folds the sidebar to its rail whatever is chosen.
pub const SIDEBAR: [(SidebarPref, &str); 3] = [
    (SidebarPref::Auto, "Automatic"),
    (SidebarPref::Expanded, "Labels"),
    (SidebarPref::Collapsed, "Rail"),
];

pub const SIDEBAR_NOTE: &str =
    "A narrow window always folds the sidebar to its rail; this choice applies when there is room.";

/// Where this device's data comes from now, and the switch to the other source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSource {
    pub line: &'static str,
    pub label: &'static str,
    pub target: DataState,
}

/// `configured`: a native socket or a stored browser pairing exists to return to.
pub fn data_source(data: DataState, configured: bool) -> DataSource {
    match data {
        DataState::Fixture => DataSource {
            line: "Showing the sample-data preview. Nothing here comes from or is sent to corrald.",
            label: "Leave the preview",
            target: if configured {
                DataState::Daemon
            } else {
                DataState::Disconnected
            },
        },
        DataState::Daemon => DataSource {
            line: "Showing live data from corrald.",
            label: PREVIEW,
            target: DataState::Fixture,
        },
        DataState::Disconnected => DataSource {
            line: "Not connected, so nothing is shown until this device connects. The preview shows labelled sample data instead.",
            label: PREVIEW,
            target: DataState::Fixture,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::devices::BrowserPairing;

    #[test]
    fn link_prefilled_pairing_says_press_pair_not_paste() {
        let mut feed = Feed {
            pairing_code: Some("corral-pair-1.abc".into()),
            ..Feed::default()
        };
        let guide = pair_guide(&feed, PairClient::Browser, Some("http://127.0.0.1:8790"));
        assert!(!guide.status.contains("Paste"));
        assert!(guide.commands[0].1.contains("http://127.0.0.1:8790"));
        feed.pairing_code = None;
        assert!(
            pair_guide(&feed, PairClient::Browser, None)
                .status
                .contains("Paste")
        );
        feed.browser_pairing = BrowserPairing::Claimed {
            sas: "123 456".into(),
        };
        assert_eq!(
            pair_guide(&feed, PairClient::Browser, None).sas.as_deref(),
            Some("123 456")
        );
    }

    #[test]
    fn every_disconnected_destination_offers_connection_and_preview() {
        for client in [Some(PairClient::Browser), Some(PairClient::Native), None] {
            let view = unavailable(&Feed::default(), client);
            assert!(!view.connect.is_empty() && !view.reason.is_empty());
            assert_eq!(view.preview, PREVIEW);
        }
    }
}
