//! `cards-ext`: the `cards` Synaps extension's process (`docs/design.md` §1).
//!
//! The host spawns it from the plugin root (`.synaps-plugin/plugin.json`,
//! `extension.command`) and speaks the extension protocol on stdio
//! ([`rpc`]). It registers `ui` in `initialize`, which the model sees as
//! `cards_ui`, and answers each `tool.call` at once ([`tool`]): a card that
//! is showing, or a refusal. With the manifest's `blocks` config on, it
//! registers `block` too, the model's `cards_block` ([`block`],
//! `docs/blocks.md`): a batch of block ops validated, or refused. It needs
//! only `tools.register`: no hooks, no conversation access. One process
//! serves every session of a daemon, so the only state, the rate counters,
//! is kept per chat.
//!
//! The exit code is 0 at the end of input or after `shutdown`, and 1 when
//! the input broke off mid-frame (the host then starts it again).

mod block;
mod rpc;
mod tool;

use std::io;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut cards = tool::Cards::default();
    match rpc::serve(&mut input, &mut output, &mut cards, Instant::now) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cards: {e}");
            ExitCode::FAILURE
        }
    }
}
