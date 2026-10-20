# Feature study: Velo, OpenMarket, Hyperdash, hl.eco and Gambit

Hyperdesk's crypto features were planned from the **public descriptions** of these products
(product pages, documentation, release notes and third-party reviews, October 2026); none of
their sites were reachable from the build environment, no captures were made, and nothing of
theirs is copied. To deepen the study, see "Next steps" below.

## Velo (velo.xyz)

A crypto derivatives data and charting platform ("institutional-grade" market structure).

| Velo feature (public description) | Here | Status |
|---|---|---|
| Aggregated open interest across exchanges | `DERIV` board, OI per venue and total | Done |
| Funding rates per exchange, aggregated | `DERIV` (8h or APR, OI-weighted), per-venue columns | Done |
| Liquidations | `LIQ` live tape, long/short totals, by coin | Done (Binance, Bybit, OKX) |
| Long/short ratio | `DERIV` column and history | Done (Binance accounts) |
| Basis / futures premium | Perp premium per venue; Deribit dated futures annualised basis | Done |
| CVD | `FOOT`/`HEAT` CVD strip from the live trade stream | Done (live only) |
| Options IV / DVOL | DVOL chart in coin detail | Partial (no surface or skew yet) |
| Order book heatmaps | `HEAT` | Done |
| Coverage: Binance, Bybit, OKX, Deribit, Hyperliquid, CME | All but CME | CME has no free real-time API |
| Years of 1-minute history, charting any metric | 7–30 days (Binance history endpoints) | Not done: needs a local recorder |
| News feed | `WIRE`: multi-source wire with coin and story tags | Done |
| Market dashboard | `MKT`: returns 1h–30d, CVD in dollars, OI, funding, returns heatmap, breadth | Done |
| Seasonality by hour and session | `SEASON` | Done |
| Price / OI / volume / liquidation alerts | `CALRT` (price, change, funding, OI change, liquidation size) | Done (no volume rule yet) |
| ETF flows | — | Not done (no free source) |

## OpenMarket (openmarket.xyz, formerly Kiyotaka)

A social, professional charting platform across crypto, equities and prediction markets
(4,000+ perps and spot pairs on 29 venues), with order-flow tools and a scripting layer.

| OpenMarket feature (public description) | Here | Status |
|---|---|---|
| Order book heatmap (incl. Hyperliquid HIP-3) | `HEAT` on Binance futures or Hyperliquid | Done (main Hyperliquid perps; HIP-3 not yet) |
| Footprint charts (volume at price, bid × ask) | `FOOT` with 1m/5m/15m/1h, POC, delta, CVD | Done |
| Liquidation heatmap from on-chain Hyperliquid positions | `HLPOS` liquidation map from the top 200 wallets' positions | Partial (current levels, sampled wallets) |
| TP/SL heatmap | — | Not done |
| Suites (bundles of up to 8 indicators, e.g. "Ultimate CVD", "Aggregated Derivatives") | Saved layouts; the built-in **Crypto Desk** layout | Partial |
| Prediction markets (Polymarket events, candles, depth) | `PRED`: markets, outcome history, book | Done |
| Equities | Charts and quotes (Yahoo/Alpaca) | Done |
| Scripting (kScript / "wrun") and AI-assisted indicator building | The original has 151 built-in indicators | Not done |
| Social sharing of charts | Layout import/export (JSON) | Partial |

## Hyperdash (hyperdash.com)

Hyperliquid analytics: wallet and trader tracking, liquidation maps from real positions,
cohorts, leaderboards, vaults, copy trading and TWAP orders.

| Hyperdash feature (public description) | Here | Status |
|---|---|---|
| Track any wallet's positions, PnL and liquidation prices | `HLW <address>`, saved wallets | Done |
| Alerts on tracked traders' moves | Toast when a displayed wallet opens, closes, flips or resizes | Partial (only wallets open in a pane) |
| Leaderboard / top trader discovery | `HLTOP` (24h/7d/30d/all, PnL/ROI/value/volume) | Done |
| Vaults | `VAULTS` (TVL, APR, open the vault's positions) | Done |
| Liquidation heatmap from known positions | `HLPOS` liquidation map (top 200 wallets) | Done as a current-levels map (not a time heatmap) |
| Cohort analysis by account size | `HLPOS` cohorts (Shrimp → Kraken) | Done |
| Whale positions | `HLPOS` largest positions; `PRINTS` large trades with wallets | Done |
| Copy trading, TWAP execution | — | Not done: needs order signing (below) |

## hl.eco

The Hyperliquid ecosystem site: 50+ dashboards, an ecosystem directory, a HyperEVM/HyperCore
explorer and a financials (revenue) dashboard.

| hl.eco feature (public description) | Here | Status |
|---|---|---|
| Ecosystem metrics dashboards | `HLECO`: perp OI/volume, HYPE, HLP, spot market caps | Done |
| HIP-3 builder markets | `HLECO` HIP-3 table (DEX, markets, OI, volume) | Done |
| Financials / revenue | `HLECO` daily fees and revenue (DefiLlama) | Done |
| Explorer, .hl names | `HLW` for HyperCore accounts | Partial (no HyperEVM transactions or .hl names) |
| Project directory | — | Not done (no public API found) |

## Gambit Terminal (terminal.gambit.zone)

Public description: a crypto and equities terminal with live charts, news, options flow,
on-chain data and a non-custodial Hyperliquid perps order ticket. Gambit marks its software,
design and layout as proprietary, so only that one-line feature list was used.

| Gambit feature | Here | Status |
|---|---|---|
| Live charts, equities and crypto | Charts, `WEI`, `OMON` | Done |
| News | `WIRE` news wire; `CALLS` KOL calls from X | Done (X needs your token or RSS bridges) |
| Options flow | `OFLOW`: Deribit BTC/ETH trades, blocks, premiums, put/call | Done for crypto; no free US equity flow source |
| On-chain data | Hyperliquid wallets, positioning and prints | Done (Hyperliquid) |
| Non-custodial Hyperliquid order ticket | — | Not done (below) |

### A safe path to Hyperliquid order entry

Orders on Hyperliquid are signed (EIP-712 over a msgpack hash of the action) by the account
or by an **API ("agent") wallet** the account approves. The safe design, mirroring the existing
SnapTrade trading flow:

1. The user creates an agent wallet in the Hyperliquid app (it can trade but not withdraw)
   and pastes its private key into Settings, stored in the OS keychain / Credential Manager,
   never in layouts or logs.
2. Trading stays off until explicitly enabled; every order is shown with size, notional,
   leverage and the estimated liquidation price and needs one confirmation.
3. Signing is implemented against Hyperliquid's published reference vectors (their Python
   SDK test fixtures) and exercised only against testnet (`api.hyperliquid-testnet.xyz`) in CI.

This was deliberately not built blind in this pass: a signing bug places or rejects real
orders, and the build environment could not reach Hyperliquid to test it.

## Next steps (suggested order)

1. **Recorder**: persist funding/OI snapshots and trades to SQLite (the bar cache already
   exists) so history goes beyond what the venues serve and footprints survive restarts.
2. **Derivatives on the chart**: OI, funding, CVD and liquidation markers as chart sub-panes
   (the chart already supports oscillator panes; feed them from `hd-flow`).
3. **Spot vs perp CVD** (OpenMarket's "is it spot- or perp-led" view): add Binance and
   Coinbase spot trade streams next to the perp stream.
4. **Hyperliquid HIP-3 in charts and heatmaps** (they are listed in `HLECO` today), and
   liquidation levels over every wallet rather than the top 200.
5. **Options**: Deribit chain, IV surface, skew and term structure for BTC/ETH.
6. **Behavioural study**: with a tool like REA (`npx rea-agents setup`), run its website mode on
   the public, logged-out pages of these products and add evidence-backed notes here before
   building the next feature. Study behaviour only; never copy code, assets or design.
