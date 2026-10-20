# Notice

## Credits

Hyperdesk's terminal core (command line, docking workspace, GPU charts, indicators, themes,
Congress trade tracking and the SnapTrade integration) started from
[terminal-rs](https://github.com/michaelshimeles/terminal-rs) by Michael Shimeles. Hyperdesk adds
the Windows app and installer, everything in `crates/flow` (derivatives, order flow, Hyperliquid,
options flow, market dashboard, news wire, KOL calls, prediction markets, alerts), the panes that
show them (`app/src/app/flow_view.rs`, `hl_view.rs`, `market_view.rs`, `kit.rs`,
`app/src/worker/flow.rs`), and its own name, icon and theme.

Feature ideas were studied from the public descriptions of [Velo](https://velo.xyz),
[OpenMarket](https://openmarket.xyz), [Hyperdash](https://hyperdash.com), [hl.eco](https://hl.eco)
and [Gambit Terminal](https://terminal.gambit.zone) (see [`docs/RESEARCH.md`](docs/RESEARCH.md)).
None of their code, data, branding or design is used, and Gambit's site, which marks its design as
proprietary, was not accessed. All data comes from the venues' own public APIs
([`docs/DATA_SOURCES.md`](docs/DATA_SOURCES.md)).

## Licence status

terminal-rs did not include a licence when it was imported. Without a licence, its code is "all
rights reserved" by default, so copying, modifying and redistributing it needs the author's
permission. Until that is granted (or terminal-rs adopts a licence), keep this repository private,
use it personally, and do not distribute builds. Update this notice when the situation changes.

## Data terms

Each data source has its own terms of use: for example, the Senate eFD forbids commercial use,
exchanges restrict access by country, and the X API has its own developer terms. You are
responsible for complying with them.
