# Notice and attribution

This project is a fork of **[terminal-rs](https://github.com/michaelshimeles/terminal-rs)** by
Michael Shimeles (imported from commit `d6006d6`), extended with:

- a Windows build (data folder, fonts, Credential Manager secrets, icon and version resource,
  GUI subsystem with a log file, installer and CI);
- crypto derivatives, liquidations, order-flow and prediction-market panes in the new
  `crates/flow` (`tr-flow`), inspired by the public feature sets of
  [Velo](https://velo.xyz) and [OpenMarket](https://openmarket.xyz).

No code, assets, branding or data from Velo, OpenMarket, TradingView or Bloomberg is used. Their
products were only studied for what features they offer; every pane here is built on the
venues' own public APIs (see [`docs/DATA_SOURCES.md`](docs/DATA_SOURCES.md)).

## Licence status

At the time of import the upstream repository **did not include a licence file**. Without a
licence, the upstream code is "all rights reserved" by default, which means copying,
modifying and redistributing it needs the author's permission. Until the upstream author adds
a licence (or grants permission), treat this repository as for **personal, non-commercial
use**, keep it private, and do not distribute builds. If upstream later adopts a licence, its
terms apply to the upstream portions and this notice should be updated.

The new code in `crates/flow`, `app/src/app/flow_view.rs`, `app/src/worker/flow.rs`,
`app/src/platform.rs` and the Windows scripts was written for this fork.

## Data terms

Each data source has its own terms of use (for example, the Senate eFD forbids commercial use;
exchanges restrict access by country). You are responsible for complying with them.
