use super::*;

pub(super) fn messages(world: &OmoWorld) -> Vec<Message> {
    Inbox::new(&world.ctx.store, world.recipient())
        .unwrap()
        .messages()
        .unwrap()
}
