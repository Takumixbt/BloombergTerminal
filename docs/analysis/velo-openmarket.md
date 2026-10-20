# Velo and OpenMarket: feature study and what this fork implements

Upstream built its chart UX from REA ("Reverse Engineer Anything") captures of
tradingview.com. For Velo and OpenMarket this fork worked from their **public descriptions**
(product pages, user guides, release notes and third-party reviews, October 2026), because
neither site was reachable from the build environment. No captures were made and nothing of
theirs is copied; to deepen this study with REA's website mode, see "Next steps" below.

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
| ETF flows, news | News from upstream (`CN`) | ETF flows not done |
| Alerts | Upstream has Congress alerts only | Not done for derivatives |

## OpenMarket (openmarket.xyz, formerly Kiyotaka)

A social, professional charting platform across crypto, equities and prediction markets
(4,000+ perps and spot pairs on 29 venues), with order-flow tools and a scripting layer.

| OpenMarket feature (public description) | Here | Status |
|---|---|---|
| Order book heatmap (incl. Hyperliquid HIP-3) | `HEAT` on Binance futures or Hyperliquid | Done (main Hyperliquid perps; HIP-3 not yet) |
| Footprint charts (volume at price, bid × ask) | `FOOT` with 1m/5m/15m/1h, POC, delta, CVD | Done |
| Liquidation heatmap from on-chain Hyperliquid positions | — | Not done: needs position data (Hyperliquid `clearinghouseState` per address) |
| TP/SL heatmap | — | Not done |
| Suites (bundles of up to 8 indicators, e.g. "Ultimate CVD", "Aggregated Derivatives") | Saved layouts; the built-in **Crypto Desk** layout | Partial |
| Prediction markets (Polymarket events, candles, depth) | `PRED`: markets, outcome history, book | Done |
| Equities | Upstream (Yahoo/Alpaca) | Done upstream |
| Scripting (kScript / "wrun") and AI-assisted indicator building | Upstream has 151 built-in indicators | Not done |
| Social sharing of charts | Layout import/export (JSON) | Partial |

## Next steps (suggested order)

1. **Recorder**: persist funding/OI snapshots and trades to SQLite (the bar cache already
   exists) so history goes beyond what the venues serve and footprints survive restarts.
2. **Derivatives on the chart**: OI, funding, CVD and liquidation markers as chart sub-panes
   (the chart already supports oscillator panes; feed them from `tr-flow`).
3. **Spot vs perp CVD** (OpenMarket's "is it spot- or perp-led" view): add Binance and
   Coinbase spot trade streams next to the perp stream.
4. **Hyperliquid HIP-3** markets (`perpDexs`, `metaAndAssetCtxs` with `dex`), and Hyperliquid
   liquidation levels from public positions.
5. **Options**: Deribit chain, IV surface, skew and term structure for BTC/ETH.
6. **Alerts** on funding, OI change and large liquidations, reusing upstream's toast/alert UI.
7. **REA study**: with REA installed (`npx rea-agents setup`), run its website mode on the
   logged-out pages of velo.xyz and openmarket.xyz (as upstream did for TradingView) and add
   evidence-backed notes here before building the next feature. Study behaviour only.
