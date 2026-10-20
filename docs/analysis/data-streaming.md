# Data streaming

## Observed (`ev_5888cfc1`)

- **Transport**: one WebSocket to `wss://data.tradingview.com/socket.io/websocket?from=chart/&date=…&type=chart`
  carrying all chart and quote traffic (despite the path it is not Socket.IO framing), plus a separate
  `wss://pushstream.tradingview.com/message-pipe-ws/public` for notifications/news. REST/XHR is used for
  news, screener enums and static bundles, not for bars.
- **Framing**: each message is `~m~<len>~m~<payload>`; several messages may be packed in one WebSocket frame.
  JSON payloads are `{"m": <method>, "p": [<params…>]}`. Heartbeats are `~h~<n>` echoed back by the client.
- **Handshake**: server hello (session id, release, studies metadata hash) → client `set_auth_token`
  (`unauthorized_user_token` when logged out) → `set_locale`.
- **Two session kinds**:
  - *Chart session* `cs_…`: `chart_create_session`, `switch_timezone`, `resolve_symbol`
    (symbol + adjustment, e.g. `{"adjustment":"splits","symbol":"NASDAQ:AAPL"}`),
    `create_series(cs, series_id, "s1", sym_id, "D", 300, "")`, `create_study`, `modify_series`,
    `remove_study`, `remove_series`, `request_more_tickmarks`, `set_future_tickmarks_mode`,
    `chart_delete_session`.
  - *Quote session* `qs_…`: `quote_create_session`, `quote_set_fields` (field list), `quote_add_symbols`,
    `quote_fast_symbols` (symbols to update at high frequency — the visible ones),
    `quote_remove_symbols`, `quote_hibernate_all`. Separate quote sessions exist per consumer
    (main symbol "full", legend "simple", watchlist, a snapshotter for the ticker strip).
- **Server → client**: `symbol_resolved` (metadata), `series_loading` → `timescale_update` (bars) →
  `series_completed`; `series_timeframe`; `study_loading`/`study_completed`; `du` (data update deltas for
  series and studies); `qsd` (quote snapshot/delta: `{n: symbol, s: "ok", v: {…changed fields…}}`);
  `quote_completed`.
- **Bars payload**: `timescale_update` → `{series_id: {s: [{i: index, v: [time, open, high, low, close, volume]}…],
  ns, t, lbs: {bar_close_time}}}`. The initial request was **300 bars** (indices 0–299). Study points use
  negative/offset indices on the same time index.
- **Update cadence**: the legend button reads "One update every 5 seconds" for the logged-out overnight feed
  (BOATS / Cboe One), i.e. throttled deltas; quote deltas only include changed fields.
- 712 HTTP requests in a 9 s cold load, almost all static JS/CSS bundles; the chart is usable before
  the long tail completes.

## Inferred / community-documented

- Scrolling left past loaded history triggers a `request_more_data(cs, series_id, n)` on the chart session,
  answered by another `timescale_update` prepending bars; the client keeps one shared time index.
- `quote_fast_symbols` is updated as panels scroll, so off-screen symbols drop to a slower cadence.

## terminal-rs implications (provider trait)

```rust
trait MarketData: Send + Sync {
    async fn history(&self, symbol, timeframe, end: Option<i64>, limit: usize) -> Result<Vec<Bar>>;
    async fn quote(&self, symbols: &[String]) -> Result<Vec<Quote>>;
    fn stream(&self, symbols, timeframe) -> Option<BoxStream<Update>>; // optional
}
```

- Fetch a bounded window first (a few hundred to a few thousand bars) so first paint is instant, then page
  older history in the background when the viewport's left edge comes within ~1 screen of loaded data.
- Merge updates as deltas into the last bar (same timestamp → replace, newer → append), which is what
  `Series::merge` does.
- Separate "fast" (visible) and "slow" (watchlist) quote polling, mirroring `quote_fast_symbols`.
- All network work on tokio tasks; the UI thread only receives finished `Vec<Bar>`/quote deltas through a
  channel and never awaits.
