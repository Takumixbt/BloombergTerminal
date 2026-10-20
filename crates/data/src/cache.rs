//! SQLite cache of bars so the terminal can paint instantly from disk on cold start
//! and refresh from the network afterwards.

use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, params};
use tr_core::{Bar, Timeframe};

pub struct BarCache {
    conn: Connection,
}

impl BarCache {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS bars (
                symbol TEXT NOT NULL, tf TEXT NOT NULL, ts INTEGER NOT NULL,
                o REAL NOT NULL, h REAL NOT NULL, l REAL NOT NULL, c REAL NOT NULL, v REAL NOT NULL,
                PRIMARY KEY (symbol, tf, ts)
            ) WITHOUT ROWID;",
        )?;
        Ok(Self { conn })
    }

    pub fn store(&mut self, symbol: &str, tf: Timeframe, bars: &[Bar]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut st = tx.prepare_cached(
                "INSERT INTO bars (symbol, tf, ts, o, h, l, c, v) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (symbol, tf, ts) DO UPDATE SET o = excluded.o, h = excluded.h, l = excluded.l,
                   c = excluded.c, v = excluded.v",
            )?;
            for b in bars {
                st.execute(params![symbol, tf.label(), b.ts, b.open, b.high, b.low, b.close, b.volume])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The most recent `limit` cached bars, ascending.
    pub fn load(&self, symbol: &str, tf: Timeframe, limit: usize) -> Result<Vec<Bar>> {
        let mut st = self.conn.prepare_cached(
            "SELECT ts, o, h, l, c, v FROM bars WHERE symbol = ?1 AND tf = ?2 ORDER BY ts DESC LIMIT ?3",
        )?;
        let mut bars: Vec<Bar> = st
            .query_map(params![symbol, tf.label(), limit as i64], |r| {
                Ok(Bar::new(r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        bars.reverse();
        Ok(bars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_load_recent() {
        let mut c = BarCache::open_in_memory().unwrap();
        let bars: Vec<Bar> = (0..50).map(|i| Bar::new(i * 60, 1.0, 2.0, 0.5, 1.5, 10.0)).collect();
        c.store("AAPL", Timeframe::M1, &bars).unwrap();
        c.store("AAPL", Timeframe::M1, &[Bar::new(49 * 60, 1.0, 3.0, 0.5, 2.5, 11.0)]).unwrap();
        let got = c.load("AAPL", Timeframe::M1, 10).unwrap();
        assert_eq!(got.len(), 10);
        assert_eq!(got[0].ts, 40 * 60);
        assert_eq!(got[9].close, 2.5);
        assert!(c.load("AAPL", Timeframe::D1, 10).unwrap().is_empty());
    }
}
