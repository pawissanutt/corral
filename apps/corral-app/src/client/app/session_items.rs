use super::*;
use crate::model::session_feed::ItemFetch;

const MAX_ITEM_BYTES: u64 = 4 * 1024 * 1024;
const MAX_READY_ITEMS: usize = 64;

pub(super) struct ItemAssembly {
    session: String,
    root: String,
    token: String,
    bytes: Option<u64>,
    json: String,
}

impl ItemAssembly {
    fn request(&self, id: String) -> Outgoing {
        out(
            DETAIL_GET,
            value(DetailGet {
                target: DetailRef::SessionItem {
                    id: self.session.clone(),
                    token: self.token.clone(),
                },
                etag: None,
                page: None,
            }),
            Some(id),
        )
    }

    fn append(&mut self, page: SessionItemPage) -> Result<Option<SessionItem>, String> {
        if page.bytes > MAX_ITEM_BYTES {
            return Err("Transcript item exceeds the 4 MiB cache limit.".into());
        }
        if self.bytes.is_some_and(|bytes| bytes != page.bytes)
            || page.offset != self.json.len() as u64
            || page.offset.saturating_add(page.json.len() as u64) > page.bytes
        {
            return Err(
                "Transcript item continuation has inconsistent byte offsets or size.".into(),
            );
        }
        self.bytes = Some(page.bytes);
        if page.next.is_some()
            && (page.json.is_empty()
                || page.offset.saturating_add(page.json.len() as u64) == page.bytes)
        {
            return Err(
                "Transcript item continuation makes no progress or exceeds its size.".into(),
            );
        }
        self.json.push_str(&page.json);
        if let Some(next) = page.next {
            self.token = next;
            return Ok(None);
        }
        if self.json.len() as u64 != page.bytes {
            return Err("Transcript item continuation ended before its declared size.".into());
        }
        serde_json::from_str(&self.json)
            .map(Some)
            .map_err(|error| format!("Invalid complete transcript item: {error}"))
    }
}

impl AppState {
    pub(super) fn session_item(
        &mut self,
        feed: Signal<Feed>,
        session: String,
        token: String,
    ) -> Vec<Outgoing> {
        if feed.with_untracked(|feed| {
            matches!(
                feed.items.get(&token),
                Some(ItemFetch::Loading | ItemFetch::Ready(_))
            )
        }) {
            return vec![];
        }
        if !self.ready {
            feed.update(|feed| {
                feed.items
                    .insert(token, ItemFetch::Failed(offline().message));
            });
            return vec![];
        }
        let assembly = ItemAssembly {
            session,
            root: token.clone(),
            token: token.clone(),
            bytes: None,
            json: String::new(),
        };
        let id = requests::new_id();
        let outgoing = assembly.request(id.clone());
        self.item_fetches.insert(id, assembly);
        feed.update(|feed| {
            feed.items.insert(token, ItemFetch::Loading);
        });
        vec![outgoing]
    }

    pub(super) fn resume_items(&mut self) -> Vec<Outgoing> {
        self.item_fetches
            .iter_mut()
            .map(|(id, assembly)| {
                // An interrupted chain must restart at zero; tokens are opaque and may now refuse.
                assembly.token.clone_from(&assembly.root);
                assembly.json.clear();
                assembly.bytes = None;
                assembly.request(id.clone())
            })
            .collect()
    }

    pub(super) fn session_item_reply(
        &mut self,
        feed: Signal<Feed>,
        tag: Option<String>,
        session: &str,
        token: &str,
        outcome: Result<Value, (corral_protocol::rpc::ErrorCode, String)>,
    ) -> Result<Vec<Outgoing>, String> {
        let Some(id) = tag else {
            return Err("transcript item reply has no identity".into());
        };
        let Some(assembly) = self.item_fetches.get(&id) else {
            return Ok(vec![]);
        };
        if assembly.session != session || assembly.token != token {
            return Ok(vec![]);
        }
        let result = outcome
            .map_err(|(_, message)| message)
            .and_then(decode::<DetailReply>)
            .and_then(|reply| match reply.body {
                Some(DetailBody::SessionItem(page)) if reply.next.is_none() => Ok(page),
                _ => Err("Daemon returned an invalid transcript item page.".into()),
            });
        let Some(mut assembly) = self.item_fetches.remove(&id) else {
            return Ok(vec![]);
        };
        match result.and_then(|page| assembly.append(page)) {
            Ok(None) => {
                let outgoing = assembly.request(id.clone());
                self.item_fetches.insert(id, assembly);
                Ok(vec![outgoing])
            }
            Ok(Some(item)) => {
                self.item_ready.push_back(assembly.root.clone());
                feed.update(|feed| {
                    feed.items.insert(assembly.root, ItemFetch::Ready(item));
                    while self.item_ready.len() > MAX_READY_ITEMS {
                        if let Some(oldest) = self.item_ready.pop_front() {
                            feed.items.remove(&oldest);
                        }
                    }
                });
                Ok(vec![])
            }
            Err(reason) => {
                feed.update(|feed| {
                    feed.items.insert(assembly.root, ItemFetch::Failed(reason));
                });
                Ok(vec![])
            }
        }
    }
}
