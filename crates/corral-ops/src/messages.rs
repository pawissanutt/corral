//! Operation diagnostics go to stderr in the CLI, or to the terminal owner's channel.

use std::cell::RefCell;
use std::sync::mpsc::Sender;

/// One diagnostic, keyed by operation and task so the board can suppress repeats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub task: Option<String>,
    pub source: &'static str,
    pub text: String,
}

thread_local! {
    static SINK: RefCell<Option<Sender<Message>>> = const { RefCell::new(None) };
}

/// Restores the previous sink on this thread when the terminal owner leaves scope.
pub struct MessageGuard {
    previous: Option<Sender<Message>>,
    // A thread-local sink must be restored on the thread that installed it.
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl Drop for MessageGuard {
    fn drop(&mut self) {
        SINK.with(|sink| *sink.borrow_mut() = self.previous.take());
    }
}

/// Captures diagnostics on the current thread, including nested operations and config reloads.
/// Install separately on workers; a disconnected sink never falls back to the owned terminal.
pub fn capture_messages(sender: Sender<Message>) -> MessageGuard {
    let previous = SINK.with(|sink| sink.replace(Some(sender)));
    MessageGuard {
        previous,
        _thread: std::marker::PhantomData,
    }
}

pub(crate) fn report(task: Option<&str>, source: &'static str, text: String) {
    SINK.with(|sink| match sink.borrow().as_ref() {
        Some(sender) => {
            // The UI may have exited while a worker is finishing its final operation.
            let _ = sender.send(Message {
                task: task.map(str::to_owned),
                source,
                text,
            });
        }
        None => eprintln!("corral: {text}"),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn nested_sinks_restore_the_terminal_owner() {
        // Given a terminal owner and a nested capture.
        let (tx, rx) = mpsc::channel();
        let _outer = capture_messages(tx);
        let (inner_tx, inner_rx) = mpsc::channel();
        {
            let _inner = capture_messages(inner_tx);
            // When nested operations report diagnostics.
            report(Some("T-001"), "sync", "inner".into());
        }
        report(Some("T-002"), "gate", "outer".into());
        // Then each owner gets only its own diagnostics.
        assert_eq!(inner_rx.try_recv().unwrap().text, "inner");
        assert_eq!(rx.try_recv().unwrap().text, "outer");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn disconnected_sink_does_not_fall_back_to_stderr() {
        let (tx, rx) = mpsc::channel();
        let _guard = capture_messages(tx);
        drop(rx);
        report(None, "sync", "owner has exited".into());
    }
}
