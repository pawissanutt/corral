//! One paging identity and one geometry namespace per mounted scroll surface.

use std::cell::RefCell;
use std::rc::Rc;

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::Id;
use crate::model::daemon::{Feed, ReaderIntent, ReaderPage, ReaderStep};
use crate::model::session::{
    Action,
    transcript::{self, Transcript},
};
use crate::state::AppState;

use super::{anchors, run};

#[derive(Clone)]
pub(crate) struct Reader {
    app: AppState,
    pub(super) scroller: ElementRef,
    pub(super) holder: u64,
    bound: Rc<RefCell<Option<ReaderIntent>>>,
    offset: Rc<RefCell<Option<f32>>>,
    pub(super) back: Signal<u32>,
}

impl Reader {
    pub(crate) fn new(app: AppState) -> Self {
        let scroller = element_ref();
        let holder = anchors::view(scroller);
        let app = untracked(|| app.scope());
        let bound = Rc::new(RefCell::new(None::<ReaderIntent>));
        let dispose = bound.clone();
        cleanup(move || {
            if let Some(intent) = dispose.take() {
                app.transcript_step(intent);
            }
        });
        Self {
            app,
            scroller,
            holder,
            bound,
            offset: Rc::default(),
            back: signal(0),
        }
    }

    pub(crate) fn geometry(&self, scope: &str) -> String {
        format!("reader:{}:{scope}", self.holder)
    }

    fn identity(&self, feed: &Feed, scope: &Id) -> Option<ReaderIntent> {
        let scope = scope.to_string();
        let cache = feed.detail(&DetailRef::Session { id: scope.clone() });
        let resolved = transcript::page(cache)?.id.clone();
        Some(ReaderIntent {
            holder: self.holder,
            store: feed.store.clone(),
            scope,
            resolved,
            anchor: None,
            step: ReaderStep::Close,
        })
    }

    fn page<'a>(&self, feed: &'a Feed, id: &ReaderIntent) -> Option<&'a ReaderPage> {
        let page = feed.transcript_windows.get(&self.holder)?;
        (page.scope == id.scope && transcript::page(Some(&page.cache))?.id == id.resolved)
            .then_some(page)
    }

    pub(crate) fn status(&self, scope: &Id) -> (bool, bool, bool) {
        self.app.feed.with(|feed| {
            let page = self
                .identity(feed, scope)
                .and_then(|id| self.page(feed, &id));
            page.map_or((false, false, false), |p| (true, p.newer, p.cache.loading))
        })
    }

    pub(super) fn scrolled(&self, offset: f32) {
        self.offset.replace(Some(offset));
    }

    pub(super) fn identity_key(&self, scope: &Id) -> Option<(Option<String>, String)> {
        self.app
            .feed
            .with(|feed| self.identity(feed, scope).map(|id| (id.store, id.resolved)))
    }

    pub(super) fn step(&self, scope: &Id, step: ReaderStep) -> bool {
        let bound = self.bound.borrow().clone();
        if let Some(mut intent) = bound {
            if intent.scope != scope.as_str() {
                return true;
            }
            let geometry = self.geometry(scope.as_str());
            let offset = self.offset.borrow().unwrap_or(0.0);
            let source = anchors::source_at(&geometry, offset);
            let anchor = self.app.feed.with_untracked(|feed| {
                let current = self.identity(feed, scope)?;
                if current.store != intent.store || current.resolved != intent.resolved {
                    return None;
                }
                let cache = self.page(feed, &intent).map(|p| &p.cache).or_else(|| {
                    feed.detail(&DetailRef::Session {
                        id: intent.scope.clone(),
                    })
                });
                let anchor = source.filter(|source| {
                    transcript::page(cache).is_some_and(|page| {
                        page.messages
                            .iter()
                            .any(|message| transcript::anchor(message) == *source)
                    })
                });
                Some(anchor)
            });
            let Some(anchor) = anchor else { return true };
            intent.anchor = anchor;
            if matches!(step, ReaderStep::Older { .. } | ReaderStep::Newer) {
                anchors::scrolled(&geometry, self.holder, offset);
            }
            intent.step = step;
            self.app.transcript_step(intent)
        } else {
            false
        }
    }

    pub(super) fn older(&self, cursor: Option<String>, fallback: Action) {
        self.back.update(|n| *n = n.wrapping_add(1));
        let handled = match &fallback {
            Action::Older(scope) => {
                cursor.is_some_and(|cursor| self.step(scope, ReaderStep::Older { cursor }))
            }
            _ => false,
        };
        if !handled {
            run(self.app, fallback);
        }
    }

    pub(super) fn newer(&self, scope: &Id, fallback: Option<Action>) {
        if !self.step(scope, ReaderStep::Newer)
            && let Some(action) = fallback
        {
            run(self.app, action);
        }
    }

    pub(crate) fn bind(&self, base: Memo<Option<Transcript>>) -> Memo<Option<Transcript>> {
        let reader = self.clone();
        let identity = computed(move || {
            let scope = base.with(|base| base.as_ref().map(|t| t.scope.clone()))?;
            reader.app.feed.with(|feed| reader.identity(feed, &scope))
        });
        let reader = self.clone();
        effect(move || {
            let id = identity.get();
            if reader
                .bound
                .borrow()
                .as_ref()
                .map(|b| (&b.store, &b.scope, &b.resolved))
                == id.as_ref().map(|b| (&b.store, &b.scope, &b.resolved))
            {
                return;
            }
            reader.offset.take();
            if let Some(old) = reader.bound.replace(id) {
                untracked(|| reader.app.transcript_step(old));
            }
        });
        let reader = self.clone();
        let fetches = computed(move || {
            let Some(id) = identity.get() else {
                return Vec::new();
            };
            reader.app.feed.with(|feed| {
                let Some(page) = reader.page(feed, &id) else {
                    return Vec::new();
                };
                reader.app.session.with(|state| {
                    transcript::window_fetches(feed, Some(&page.cache), state.ui(&id.scope))
                        .into_iter()
                        .map(|token| Action::Fetch {
                            session: Id::from(id.scope.as_str()),
                            token,
                        })
                        .collect::<Vec<_>>()
                })
            })
        });
        let app = self.app;
        effect(move || {
            for fetch in fetches.get() {
                untracked(|| run(app, fetch));
            }
        });
        let reader = self.clone();
        computed(move || {
            let window = identity.get().and_then(|id| {
                reader.app.feed.with(|feed| {
                    let page = reader.page(feed, &id)?;
                    Some(reader.app.session.with(|state| {
                        transcript::reader_transcript(
                            &Id::from(id.scope.as_str()),
                            state.ui(&id.scope),
                            &page.cache,
                            feed,
                        )
                    }))
                })
            });
            window.or_else(|| base.get())
        })
    }
}
