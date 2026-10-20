<p align="center">
  <img src="app/assets/icon.png" width="96" alt="Hyperdesk icon">
</p>

<h1 align="center">Hyperdesk</h1>

<p align="center">
  <b>A keyboard-driven trading terminal for crypto and markets.</b><br>
  Cross-venue derivatives, order flow, Hyperliquid on-chain intelligence, news, KOL calls,
  prediction markets and GPU charts in one dense, dockable desk. Native on Windows, macOS and
  Linux, written in Rust, with no account and no API keys needed.
</p>

<p align="center">
  <img src="docs/img/demo.gif" alt="Hyperdesk demo: the Crypto Desk with a live order book heatmap, footprint and liquidations, then the Market Desk, the Hyperliquid Desk and a whale's wallet" width="100%">
</p>

<p align="center"><sub>Offline demo data (<code>HD_PROVIDER=sample</code>). Everything you see runs live against public exchange APIs.</sub></p>

---

## What's on the desk

Type a command anywhere (it lands in the command line), press <kbd>Ctrl</kbd>+<kbd>K</kbd> for
the palette, or add any panel from a pane's <kbd>+</kbd> menu. Panes dock, tab, split, pop out
and link by colour, so one coin change drives the chart, heatmap and footprint together.

| | Command | What you get |
|---|---|---|
| **Markets** | `MKT` · `MKT HEAT` | Returns over 1h / 4h / 24h / 7d / 30d, 24h taker-flow **CVD in dollars** with a sparkline, open interest and funding for the top 40 coins; a returns **heatmap treemap** sized by OI; market breadth |
| | `SEASON BTC` | **Seasonality**: average hourly return by weekday × hour (UTC) over ~125 days, plus Asia / Europe / US / late-US sessions |
| | `CALRT BTC > 70k` | **Alerts** on price, 24h change, funding, OI change or liquidation size (`SOL CHG > 8%`, `HYPE FUNDING > 0.05%`, `ETH OI < -5%`, `LIQ > 1M`) |
| **Derivatives** | `DERIV` | Every USDT perp across **Binance, Bybit, OKX, Hyperliquid** (+ Deribit): aggregated OI, OI-weighted funding (8h or APR), funding per venue, premium, long/short; sortable heat-shaded board |
| | `DERIV ETH` | One coin: per-venue mark, index, premium, funding and countdown; OI, funding, long/short and taker-flow history; **Deribit futures curve** (annualised basis) and **DVOL** |
| | `LIQ` | Live **liquidations** from Binance, Bybit and OKX: longs vs shorts rekt over 5m–24h, size filter, top coins |
| **Order flow** | `HEAT` · `HEAT ETH HL` | **Order book heatmap**: resting liquidity over time from a locally synced book, best bid/ask, trade bubbles, live book profile, CVD |
| | `FOOT` · `CVD` | **Footprint** candles: sell × buy volume at each price, imbalance shading, POC, delta row and CVD; 1m–1h |
| | `OFLOW` | **Options flow** (Deribit BTC/ETH): every print with premium, IV and BLOCK flag; calls/puts bought vs sold, put/call, top strikes and expiries |
| **Hyperliquid** | `HLW 0x…` | **Wallet tracker**: account value, PnL, leverage; positions with entry, liquidation price and distance to it; fills; your own labels plus auto-tags (Whale, Top 10, Top 30d PnL, High leverage, Near liquidation); alerts when a wallet opens, closes or flips |
| | `HLTOP` · `VAULTS` | **Leaderboard** by PnL, ROI, value or volume over 24h–all time; vaults by TVL and APR |
| | `HLPOS BTC` | **Top-wallet positioning**: long vs short among the 200 largest accounts, size cohorts (Shrimp → Kraken), a **liquidation map** from their real liquidation prices, largest positions |
| | `PRINTS` | **Whale prints**: $25K+ Hyperliquid trades with the taker's and maker's wallets |
| | `HLECO` | **Ecosystem**: perp OI and volume, HYPE, **HIP-3 builder DEXs** and their markets, spot tokens by market cap, the HLP vault, daily **fees and revenue** |
| **News & social** | `WIRE` · `WIRE SOL` | **News wire** from CoinDesk, The Block, Cointelegraph, Decrypt, Blockworks, the Fed, the SEC and more: coin chips, story tags (HACK, REG, ETF, LIST, MACRO, HL, RAISE), important stories highlighted |
| | `CALLS` | **KOL calls** from X accounts you follow: coin, side and levels from each post, price at the call, move since, and a hit-rate **scorecard** |
| | `PRED` | **Prediction markets** (Polymarket): odds, history per outcome, order book |
| **Charts & TradFi** | `AAPL` · `BTCUSD CRNCY` · `US10Y GOVT` | GPU candles with 151 indicators, drawings and compare; stocks, ETFs, indices, FX, crypto, Treasuries, futures |
| | `OMON` · `WEI` · `GC` · `CN` | Option chains with greeks, world markets, the Treasury curve, headlines |
| | `CONG` · `POL PELOSI` | US Congress trades from the official STOCK Act filings, with chart markers and alerts |
| **Trading** | `PORT` · `BUY AAPL 10` | A paper-trading demo account by default; real brokerage accounts through SnapTrade (read-only until you turn trading on) |

Three ready-made layouts are created on first launch: **Crypto Desk**, **Hyperliquid Desk** and
**Market Desk** (`LAYOUT Crypto Desk`, or <kbd>Alt</kbd>+<kbd>1</kbd>…<kbd>9</kbd>).

<table>
  <tr>
    <td><img src="docs/img/crypto-desk.png" alt="Crypto Desk"></td>
    <td><img src="docs/img/hyperliquid-desk.png" alt="Hyperliquid Desk"></td>
  </tr>
  <tr>
    <td align="center"><b>Crypto Desk</b>: chart, heatmap, derivatives board, footprint, liquidations, predictions</td>
    <td align="center"><b>Hyperliquid Desk</b>: leaderboard, wallet, whale prints, positioning, options flow</td>
  </tr>
  <tr>
    <td><img src="docs/img/market-desk.png" alt="Market Desk"></td>
    <td><img src="docs/img/crypto-detail.png" alt="Coin detail and prediction market"></td>
  </tr>
  <tr>
    <td align="center"><b>Market Desk</b>: returns heatmap, news wire, KOL calls, ecosystem, seasonality, alerts</td>
    <td align="center"><b>DERIV BTC</b> and a Polymarket market</td>
  </tr>
</table>

## Install

**Windows 10/11 (x64).** Download `hyperdesk-<version>-setup-x64.exe` (per-user install, no admin
rights, Start menu shortcut) or the portable `hyperdesk-<version>-windows-x64.zip` from the
Releases page or the CI artifacts. Push a `v*` tag to build and publish a release.

**From source** (Windows, macOS or Linux) with [Rust](https://rustup.rs):

```sh
cargo run --release                         # live data
HD_PROVIDER=sample cargo run --release      # fully offline demo data
```

On Windows, `scripts\package-windows.ps1` builds the zip and, with
[Inno Setup 6](https://jrsoftware.org/isinfo.php), the installer. From Linux,
`scripts/build-windows-cross.sh` cross-compiles the Windows app (mingw-w64).
`scripts/bundle-macos.sh` wraps a macOS build as `Hyperdesk.app`. Linux needs the usual X11 or
Wayland libraries and a Vulkan or GL driver.

## Optional setup

Nothing below is required; every pane works without it.

| To get | Do this |
|---|---|
| **KOL calls from X** | X has no free read API. Either set `X_BEARER_TOKEN` (a paid X API tier) before starting Hyperdesk, or give each account an RSS bridge URL (RSSHub, Nitter, rss.app) when you add it in the `CALLS` pane. Calls are kept in `kol_calls.json` so their scorecard survives restarts. |
| **Your own news feeds** | Put `[{"name": "My feed", "url": "https://…/rss"}]` in `news_feeds.json` in the data folder (RSS or Atom). It replaces the default list. |
| **Wallet labels** | Click **✎ Label** in any `HLW` wallet. Labels are saved in `wallet_labels.json`; vault names and leaderboard names are added automatically. |
| **Stocks from Alpaca** | `APCA_API_KEY_ID` and `APCA_API_SECRET_KEY` (free tier). |
| **Brokerage accounts** | **Settings › Brokerage**: SnapTrade client ID and consumer key, **Register User**, **Connect Brokerage…**. Secrets go to the Windows Credential Manager or macOS Keychain. Live order entry stays off until you enable it in **Settings › Trading**, and every order is previewed and confirmed. |

**Where things live:** `%APPDATA%\Hyperdesk` on Windows, `~/Library/Application Support/Hyperdesk`
on macOS, `$XDG_DATA_HOME/hyperdesk` on Linux (or `HD_DATA_DIR`). Layouts, themes, prefs, caches,
labels, alerts and KOL calls are plain JSON/TOML/SQLite files there. Release builds on Windows
log to `hyperdesk.log` in the same folder.

## Where the data comes from

Everything is fetched directly from public sources on your machine; there is no Hyperdesk server.
Binance, Bybit, OKX, Hyperliquid and Deribit public REST and websocket APIs; Polymarket's Gamma
and CLOB APIs; DefiLlama's fees API; publishers' RSS feeds; Yahoo Finance, Coinbase,
Treasury.gov and FRED; the Senate eFD and House Clerk for Congress trades. The full list, endpoint
by endpoint, with refresh rates and caveats (geo-blocks, rate limits, unofficial endpoints), is in
[`docs/DATA_SOURCES.md`](docs/DATA_SOURCES.md).

## Commands and keys

Press <kbd>F1</kbd> (or `HELP`) for the searchable list; every hotkey can be rebound in
**Settings › Hotkeys**.

| Keys | Action |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>K</kbd> | command palette: commands, tickers, panels, layouts, themes, settings |
| <kbd>Tab</kbd> · <kbd>Shift</kbd>+<kbd>Tab</kbd> | next · previous pane |
| <kbd>Ctrl</kbd>+<kbd>D</kbd> · <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> · <kbd>Ctrl</kbd>+<kbd>T</kbd> · <kbd>Ctrl</kbd>+<kbd>W</kbd> · <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> | split right · split down · new tab · close pane · pop out |
| <kbd>Ctrl</kbd>+<kbd>S</kbd> · <kbd>Alt</kbd>+<kbd>1</kbd>…<kbd>9</kbd> | save layout · switch layout |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>T</kbd> · <kbd>Ctrl</kbd>+<kbd>,</kbd> | cycle theme · settings |
| <kbd>F1</kbd>–<kbd>F9</kbd> | help, Congress, politicians, alerts, watchlist, refresh, portfolio, positions, orders |
| wheel · drag · double-click | zoom · pan · reset (charts, heatmap, footprint) |

| Command | Action |
|---|---|
| `BTCUSD CRNCY` · `AAPL` · `SPX INDEX` · `US10Y GOVT` · `CL1 CMDTY` | load a security |
| `5` `1H` `D` · `RSI` `MACD` `SMA 200` · `IND` · `CMP ETH` · `LOG` | interval · indicators · picker · compare · log scale |
| `DERIV [COIN]` · `LIQ [COIN]` · `HEAT [COIN] [HL]` · `FOOT [COIN]` · `OFLOW [ETH]` | derivatives and order flow |
| `HLW 0x…` · `HLTOP` · `VAULTS` · `HLPOS [COIN]` · `PRINTS [COIN]` · `HLECO` | Hyperliquid |
| `MKT` · `MKT HEAT` · `SEASON [COIN]` · `CALRT [RULE]` | market dashboard, seasonality, alerts |
| `WIRE [COIN]` · `CALLS` · `PRED [QUERY]` · `CN` | news wire, KOL calls, prediction markets, headlines |
| `OMON` · `WEI` · `GC` · `SECF <query>` · `CONG` · `POL <name>` | options, world markets, curve, search, Congress |
| `PORT` · `POS` · `ORD` · `BUY AAPL 10` · `SELL ALL TSLA` · `DEMO` | portfolio and trading |
| `NEW <PANEL>` · `SPLIT DOWN <PANEL>` · `LINK BLUE` · `TITLE <name>` · `POP` · `CLOSE` | arrange panes |
| `LAYOUT <name>` · `LAYOUT SAVE <name>` · `LAYOUT 4` · `THEME <name>` | layouts and themes |

## Themes

Nine built in, led by **Hyperdesk** (gold tickers on near-black with a mint accent): Terminal
Dark and Light, Amber Terminal (amber monospace on black), High Contrast, Solarized Dark and Light,
Nord and Monokai. **Settings › Themes** is a live editor; custom themes are TOML files you can
share.

## Architecture

```
app/                 eframe shell: command line, docked panes, panels, settings
  src/app/           workspace, panels, flow_view (derivatives, order flow, predictions),
                     hl_view (Hyperliquid), market_view (market, news, KOL, eco, alerts), kit (shared UI)
  src/worker*        tokio runtime: every network, SQLite and indicator job (the UI never blocks)
crates/core          bars, timeframes, securities catalog, 151 indicators, sample data
crates/chart         wgpu renderer with level-of-detail pyramids (1M+ bars at full frame rate)
crates/data          stocks, FX, rates and crypto spot providers, websocket feeds, SQLite bar cache
crates/flow          crypto: derivatives board, history, Deribit, liquidations, order flow
                     (synced local book, heatmap, footprint, CVD), Hyperliquid (wallets,
                     leaderboard, positioning, prints, ecosystem), options flow, market dashboard,
                     seasonality, alerts, news wire, KOL calls, Polymarket, offline sample mode
crates/congress      Senate eFD and House Clerk parsers, rosters, SQLite store, alerts
crates/broker        SnapTrade client, order entry, portfolio model, secret storage
crates/macos         native macOS window chrome
```

Every venue parser in `crates/flow` is a pure function over the venue's JSON, tested against
fixtures; streams hand the UI a shared handle and stop by themselves when the last pane using
them closes.

## Development

```sh
cargo test --workspace                          # unit, fixture and app tests
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p hd-flow --test live -- --ignored  # live checks against the real venues
cargo test -p hd-data -- --ignored              # live checks for the market data sources
```

CI builds, tests, lints and smoke-runs on Windows, macOS and Linux and packages the Windows app.

## Limitations

- Venues block some countries (Binance blocks the US, for example); a blocked venue shows as
  offline and the rest keep working.
- Heatmaps, footprints and CVD build live from when a pane opens; history is not backfilled.
- Hyperliquid positioning covers the 200 largest leaderboard accounts, not every wallet. The
  leaderboard and vault lists come from endpoints Hyperliquid's app uses rather than a documented
  API.
- No live Hyperliquid order entry or copy trading yet: signing orders needs a wallet key, and the
  safe design for it is in [`docs/RESEARCH.md`](docs/RESEARCH.md).
- Yahoo Finance is unofficial and rate-limited. Senate eFD terms forbid commercial use.

## Credits and licence

See [`NOTICE.md`](NOTICE.md). Feature ideas were studied from the public descriptions of Velo,
OpenMarket, Hyperdash, hl.eco and Gambit ([`docs/RESEARCH.md`](docs/RESEARCH.md)); none of their
code, data or design is used.
