# Layouts, symbol/interval entry and hotkeys

## Observed

- **Type-to-search** (`ev_16ea0e50`, step `type_m`): pressing a letter with the chart focused opens a
  "Symbol search" modal with the letter already in the box; results stream as typed, with asset-class
  filter chips (All, Stocks, Funds, Futures, Forex, Crypto, Indices, Bonds, Economy, Options); each row shows
  ticker (matched prefix highlighted), name, type and exchange.
- **Type-to-change-interval** (`type_5`): pressing a digit opens a small "Change interval" box pre-filled
  with the digit and a live interpretation underneath ("5 minutes"); Enter applies (the toolbar then reads
  `5m` and the legend `Apple Inc · 5 · NASDAQ`). `D` is shown as "1 day".
- **Layout selector** (`layout` step): a grid of pane arrangements grouped by count 1–10, 12, 14, 16, with
  multiple splits per count (columns, rows, mixed). A "Sync in layout" section toggles Symbol, Interval,
  Crosshair, Time and Date range — **Crosshair is on by default**, others off.
- **Chart type menu** (`ev_c8bc591b`): Bars, Candles (default), Hollow candles, Volume candles; Line, Line with
  markers, Step line; Area, HLC area, Baseline; Columns, High-low; Volume footprint, Time price opportunity,
  Session volume profile; Heikin Ashi, Renko, Line break, Kagi, Point & figure, Range.
- **Compare**: a "Compare symbols" button (`+` icon next to the symbol).

## Documented hotkeys

| Key | Action |
|---|---|
| any letter | symbol search |
| any digit, `,` | interval entry |
| Alt+T / Alt+H / Alt+J / Alt+V / Alt+C | trendline / horizontal line / horizontal ray / vertical line / crossline |
| Alt+F | fib retracement |
| Alt+R | reset chart view |
| Alt+A | add alert |
| Alt+I | invert scale |
| Alt+L | log scale |
| Ctrl/Cmd+Z, Ctrl/Cmd+Y | undo / redo |
| Ctrl/Cmd+K | quick search |
| Esc | cancel drawing / close dialog |
| Del / Backspace | remove selected drawing |
| Shift+F | fullscreen |
| ←/→, Ctrl+↑/↓ | pan / zoom |

## terminal-rs implications

- The Bloomberg command line is the primary input. Grammar: `<SYMBOL> [<FUNCTION>] <GO>` where `<GO>` is
  Enter, e.g. `AAPL <GO>`, `AAPL CONG <GO>` (congress trades for AAPL), `5 <GO>` (interval), `1W <GO>`,
  `RSI <GO>`, `CMP MSFT <GO>`, `POL Pelosi <GO>`, `LAYOUT 4 <GO>`.
- With focus on a chart, a letter or digit starts typing into the command line (same as TradingView's
  type-to-search); a live hint under the line interprets the input ("5 minutes", "Apple Inc").
- Tiling: presets for 1, 2 (cols/rows), 3, 4 (2×2) and 6 (3×2) panes; any pane can hold a chart or a
  terminal function (congress feed, profile, ticker list, watchlist, alerts). Crosshair sync on by default,
  symbol/interval sync optional per layout.
- Chart types for the MVP: candles, OHLC bars, line, area (others are out of scope).
