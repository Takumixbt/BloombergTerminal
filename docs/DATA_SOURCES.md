# Where the data comes from

Every number in the terminal comes from a **free, public source** called directly from your
machine. There is no hyperdesk server in between, no account, and no API key is required
(Alpaca and SnapTrade are optional and use your own keys). Nothing is resold or redistributed:
each install fetches its own data, and you are bound by each source's terms of use.

With `HD_PROVIDER=sample` nothing goes over the network; every pane shows deterministic
simulated data with a **SIMULATED** badge.

## Markets (charts, quotes, watchlist, world markets)

| What | Source | Endpoint(s) | Notes |
|---|---|---|---|
| Stocks, ETFs, indices, FX, futures: bars and quotes | Yahoo Finance (unofficial) | `query1.finance.yahoo.com/v8/finance/chart`, `/v8/finance/spark` | Unofficial and rate-limited; may break without notice |
| Stocks and ETFs (optional) | Alpaca Market Data | `data.alpaca.markets/v2/stocks` | Used instead of Yahoo when `APCA_API_KEY_ID` / `APCA_API_SECRET_KEY` are set (free tier: IEX feed) |
| Security search | Yahoo Finance | `query2.finance.yahoo.com/v1/finance/search` | |
| Option chains (`OMON`) | Yahoo Finance | `query2.finance.yahoo.com/v7/finance/options` (crumb from `fc.yahoo.com`, `/v1/test/getcrumb`) | Greeks are computed locally (Black-Scholes) |
| Crypto spot bars, quotes, live candles | Binance spot | REST `data-api.binance.vision` then `api.binance.com`; websocket `data-stream.binance.vision`, `stream.binance.com:9443` | Public market-data mirror first |
| Crypto fallback | Coinbase Exchange | REST `api.exchange.coinbase.com`; websocket `ws-feed.exchange.coinbase.com` | When Binance is unreachable (e.g. geo-blocked) |
| Order book (`OB`) | Binance spot, Coinbase fallback | `/api/v3/depth`, `/products/{id}/book` | Markets without a free book show a simulated one |
| US Treasury yield curve (`GC`) | U.S. Treasury | `home.treasury.gov/…/daily-treasury-rates.csv` | Official par yield curve |
| Treasury fallback | FRED (St. Louis Fed) | `fred.stlouisfed.org/graph/fredgraph.csv` | |
| News (`CN`) | Yahoo Finance headlines; CoinDesk RSS for crypto | `coindesk.com/arc/outboundfeeds/rss/` | |

## Congress trades (`CONG`, `POL`)

| What | Source | Endpoint(s) |
|---|---|---|
| Senate Periodic Transaction Reports | U.S. Senate eFD | `efdsearch.senate.gov` (agreement + search API + report pages) |
| House Periodic Transaction Reports | Clerk of the U.S. House | `disclosures-clerk.house.gov` (`{year}FD.zip` index + PTR PDFs) |
| Party, state, bioguide IDs | House Clerk, Senate.gov | `clerk.house.gov/xml/lists/MemberData.xml`, `senate.gov/…/senators_cfm.xml` |

These are official STOCK Act disclosure systems. Senate eFD terms prohibit commercial use and
use for credit decisions.

## Brokerage (optional)

| What | Source | Notes |
|---|---|---|
| Accounts, balances, positions, orders, activity; live order entry | SnapTrade (`api.snaptrade.com`) | Your own SnapTrade keys; read-only unless you enable trading |

## Crypto derivatives (`DERIV`) — the Velo-style features

| What | Venue | Endpoint(s) |
|---|---|---|
| Price, mark, index, funding, next funding, 24h volume and change | Binance USDⓈ-M futures | `fapi.binance.com/fapi/v1/ticker/24hr`, `/fapi/v1/premiumIndex`, `/fapi/v1/fundingInfo` (non-8h intervals) |
| Open interest | Binance | `/fapi/v1/openInterest` (one call per symbol, top contracts only) |
| Long/short account ratio | Binance | `/futures/data/globalLongShortAccountRatio` |
| Everything incl. open interest value and funding interval | Bybit | `api.bybit.com/v5/market/tickers?category=linear` |
| Price, open interest (USD), mark, funding, index | OKX | `www.okx.com/api/v5/market/tickers`, `/public/open-interest`, `/public/mark-price`, `/public/funding-rate`, `/market/index-tickers` |
| Price, mark, oracle, hourly funding, open interest, volume | Hyperliquid | `POST api.hyperliquid.xyz/info {"type":"metaAndAssetCtxs"}` |
| BTC/ETH inverse perps | Deribit | `www.deribit.com/api/v2/public/get_book_summary_by_currency` |
| Coin detail: OI, funding, long/short, taker buy/sell history (7–30 days) | Binance | `/futures/data/openInterestHist`, `/fapi/v1/fundingRate`, `/futures/data/globalLongShortAccountRatio`, `/futures/data/takerlongshortRatio` |
| Futures curve (annualised basis) and DVOL | Deribit | `get_book_summary_by_currency?kind=future`, `get_volatility_index_data` |

Funding is compared on an 8-hour basis (`rate × 8 / interval hours`; Hyperliquid funds hourly)
and the board's aggregate is weighted by each venue's open interest.

## Liquidations (`LIQ`)

| Venue | Stream | Notes |
|---|---|---|
| Binance | `wss://fstream.binance.com/ws/!forceOrder@arr` | Binance sends at most one (the largest) liquidation per contract per second, so cascades are undercounted |
| Bybit | `wss://stream.bybit.com/v5/public/linear`, topic `allLiquidation.<symbol>` | One topic per contract; the terminal follows the 60 largest |
| OKX | `wss://ws.okx.com:8443/ws/v5/public`, channel `liquidation-orders` (SWAP) | Sizes converted from contracts with `/api/v5/public/instruments` `ctVal` |

Hyperliquid has no public liquidation stream.

## Order flow (`HEAT`, `FOOT`) — the OpenMarket-style features

| Venue | Streams | Notes |
|---|---|---|
| Binance USDⓈ-M futures | `wss://fstream.binance.com/stream?streams=<sym>@aggTrade/<sym>@depth@100ms` + REST `/fapi/v1/depth?limit=1000` snapshot | Local book kept in sync by update id (`U`/`u`/`pu`) as Binance documents; resyncs on gaps |
| Hyperliquid | `wss://api.hyperliquid.xyz/ws`, `trades` + `l2Book` | `l2Book` is a 20-level snapshot per update, so the heatmap covers a narrower range |

The heatmap, footprint and CVD are built **live from the moment a pane opens** (no history is
backfilled), and kept in memory only.

## Prediction markets (`PRED`)

| What | Source | Endpoint(s) |
|---|---|---|
| Open markets, outcomes, prices, volume, liquidity | Polymarket Gamma API | `gamma-api.polymarket.com/markets` |
| Price history per outcome | Polymarket CLOB | `clob.polymarket.com/prices-history` |
| Order book | Polymarket CLOB | `clob.polymarket.com/book` |

## Hyperliquid trader intelligence (`HLW`, `HLTOP`, `HLPOS`, `PRINTS`) — Hyperdash-style

Hyperliquid runs its order book on its own chain, so every account's positions are public.

| What | Endpoint | Notes |
|---|---|---|
| A wallet's positions, margin, liquidation prices | `POST api.hyperliquid.xyz/info {"type":"clearinghouseState","user":…}` | Refreshed every 30 s while shown |
| A wallet's recent fills | `POST api.hyperliquid.xyz/info {"type":"userFills","user":…}` | Up to the 2,000 most recent |
| Leaderboard (account value; PnL, ROI, volume by window) | `GET stats-data.hyperliquid.xyz/Mainnet/leaderboard` | The list the Hyperliquid app shows; cached 10 min |
| Vaults (name, TVL, APR) | `GET stats-data.hyperliquid.xyz/Mainnet/vaults` | Cached 10 min |
| Positioning, cohorts, liquidation map | `clearinghouseState` for the 200 largest leaderboard accounts | 6 requests at a time, every 3 min; describes those wallets, not the whole market |
| Whale prints | `wss://api.hyperliquid.xyz/ws`, `trades` for the 40 most traded perps | Each trade carries `users: [buyer, seller]`; prints of $25K+ are kept |

The `stats-data` endpoints are what Hyperliquid's own web app uses rather than a documented
API, so they may change.

## Crypto options flow (`OFLOW`) — Gambit-style

| What | Endpoint | Notes |
|---|---|---|
| BTC and ETH option trades | `GET www.deribit.com/api/v2/public/get_last_trades_by_currency?kind=option` | Last 1,000 per currency, polled every 15 s and merged; `block_trade_id` marks block trades; premium = price × amount × index |

There is no free source for US equity options flow ("unusual options activity"); Yahoo's
chains (`OMON`) show volume and open interest per contract instead.

## Market dashboard, seasonality and alerts (`MKT`, `SEASON`, `CALRT`) — Velo-style

| What | Endpoint | Notes |
|---|---|---|
| Returns 1h–30d and taker-flow CVD | `GET fapi.binance.com/fapi/v1/klines?interval=1h&limit=744` for the 40 largest coins by OI | CVD = taker buy quote volume × 2 − quote volume; 6 requests at a time, cached 5 min |
| Seasonality | the same klines, two pages (~125 days) for one coin | Hourly returns grouped by UTC weekday × hour and by session |
| OI and funding columns, alert values | the derivatives board above | Alerts are checked locally every few seconds; liquidation rules use the `LIQ` stream |

## Hyperliquid ecosystem (`HLECO`) — hl.eco-style

| What | Endpoint | Notes |
|---|---|---|
| Core perps (OI, volume, HYPE) | `POST api.hyperliquid.xyz/info {"type":"metaAndAssetCtxs"}` | |
| HIP-3 builder DEXs and their markets | `{"type":"perpDexs"}`, then `metaAndAssetCtxs` with `"dex"` | |
| Spot tokens, market cap | `{"type":"spotMetaAndAssetCtxs"}` | Market cap = circulating supply × mark |
| HLP vault | the vault list above | |
| Daily fees and revenue | `GET api.llama.fi/summary/fees/hyperliquid?dataType=dailyFees` / `dailyRevenue` | DefiLlama's free API |

## News terminal (`WIRE`) — Gambit-style

| Source | Endpoint | Polled | Notes |
|---|---|---|---|
| Tweets and announcements | `GET news.treeofalpha.com/api/news?limit=100` | every 15 s | Tree of Alpha's public news feed: posts from market-moving X accounts and exchange announcements, with the coins it detected. Community-run and unofficial, so it may change or add limits |
| Binance announcements | `GET www.binance.com/bapi/composite/v1/public/cms/article/list/query?type=1` | every 60 s | The list Binance's website uses (not a documented API) |
| Bybit announcements | `GET api.bybit.com/v5/announcements/index?locale=en-US` | every 60 s | Bybit's documented public API |
| OKX announcements | `GET www.okx.com/api/v5/support/announcements` | every 60 s | OKX's documented public API |
| Outlets and regulators | RSS/Atom: CoinDesk, Cointelegraph, The Block, Decrypt, Blockworks, Bitcoin Magazine, CryptoSlate, Hyperliquid's Medium, Federal Reserve, SEC | every 90 s | |

Headlines are merged and de-duplicated. Coins come from the source's own coin detection, from
`$CASHTAGS`, tickers known to the derivatives board and common coin names; story tags (BREAKING,
LIST, HACK, REG, ETF, MACRO, HL, RAISE) come from keyword lists and the exchanges' announcement
categories. The **move since the headline** compares the board price when Hyperdesk first saw a
headline (only for headlines under 15 minutes old) with the current board price, which refreshes
about every 30 s; it is not tick-accurate. Replace the source list with your own in
`news_feeds.json` in the data folder. Check every source from your network with
`cargo test -p hd-flow --test live news -- --ignored --nocapture`.

## KOL calls (`CALLS`)

| Source | Endpoint | Notes |
|---|---|---|
| X | `GET api.x.com/2/tweets/search/recent` (`from:a OR from:b …`) | Needs your own bearer token in `X_BEARER_TOKEN`; X's free tier does not include reading |
| RSS bridges | any RSS/Atom URL per account (RSSHub, Nitter, rss.app…) | No token needed |

A post becomes a call when it names a coin and a clear direction. The price at the call is the
board price when Hyperdesk first sees the post, so calls made while Hyperdesk was closed start
from the next price it sees. Calls are stored in `kol_calls.json`.

## Things to know

- **Geo-restrictions.** Binance (`fapi.binance.com`, `api.binance.com`) refuses connections from
  some countries, including the US; Bybit and OKX restrict others. A blocked venue shows as
  offline on the board and the rest keep working. Spot crypto falls back to Coinbase.
- **Rate limits.** The derivatives board refreshes every 30 s (cached for 20 s across panes) and
  bounds per-symbol calls to the 30 most traded contracts. Yahoo is the most fragile source.
- **Delays.** Exchange data is real time; Yahoo equity quotes may be delayed by the exchange's
  rules; SnapTrade data can be cached depending on your plan.
