//! Wallet labels: a readable name and a few tags for any Hyperliquid address, from (in order)
//! your own labels (`wallet_labels.json`), vault names, leaderboard display names, and tags
//! derived from the leaderboard and the account itself (size cohort, rank, recent PnL, leverage).

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;

use crate::hyperliquid::{Account, COHORTS, Leader, Vault, cohort_of, short_address};

#[derive(Debug, Clone, Default)]
pub struct Labels {
    /// Your labels, keyed by lower-case address.
    pub user: BTreeMap<String, String>,
    names: HashMap<String, String>,
    tags: HashMap<String, Vec<String>>,
}

impl Labels {
    pub fn load(path: &std::path::Path) -> Labels {
        let user = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Labels { user, ..Default::default() }
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.user)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Set (or, with an empty label, clear) your label for `address`.
    pub fn set_user(&mut self, address: &str, label: &str) {
        let a = address.to_ascii_lowercase();
        if label.trim().is_empty() {
            self.user.remove(&a);
        } else {
            self.user.insert(a, label.trim().to_string());
        }
    }

    /// Names and tags from the leaderboard: display names, size cohort, rank by account value
    /// and the month's best and worst traders.
    pub fn absorb_leaders(&mut self, leaders: &[Leader]) {
        let mut by_month: Vec<&Leader> = leaders.iter().filter(|l| l.account_value >= 1e4).collect();
        by_month.sort_by(|a, b| b.month.pnl.total_cmp(&a.month.pnl));
        let top_month: Vec<&str> = by_month.iter().take(50).map(|l| l.address.as_str()).collect();
        let worst_month: Vec<&str> = by_month.iter().rev().take(50).map(|l| l.address.as_str()).collect();
        for (rank, l) in leaders.iter().enumerate() {
            if let Some(n) = &l.name {
                self.names.entry(l.address.clone()).or_insert_with(|| n.clone());
            }
            let mut t = vec![COHORTS[cohort_of(l.account_value)].0.split(' ').next().unwrap_or("").to_string()];
            if rank < 100 {
                t.push(format!("Top {}", if rank < 10 { 10 } else { 100 }));
            }
            if top_month.contains(&l.address.as_str()) {
                t.push("Top 30d PnL".into());
            } else if worst_month.contains(&l.address.as_str()) && l.month.pnl < 0.0 {
                t.push("Rekt 30d".into());
            }
            self.tags.insert(l.address.clone(), t);
        }
    }

    pub fn absorb_vaults(&mut self, vaults: &[Vault]) {
        for v in vaults {
            self.names.insert(v.address.clone(), format!("Vault: {}", v.name));
            self.tags.entry(v.address.clone()).or_default().push("Vault".into());
            if !v.leader.is_empty() {
                self.tags.entry(v.leader.clone()).or_default().push("Vault leader".into());
            }
        }
    }

    pub fn label(&self, address: &str) -> Option<&str> {
        let a = address.to_ascii_lowercase();
        self.user.get(&a).or_else(|| self.names.get(&a)).map(String::as_str)
    }

    /// The label, or a shortened address.
    pub fn display(&self, address: &str) -> String {
        self.label(address).map_or_else(|| short_address(address), str::to_string)
    }

    pub fn tags(&self, address: &str) -> &[String] {
        self.tags.get(&address.to_ascii_lowercase()).map_or(&[], Vec::as_slice)
    }

    /// Tags from an account snapshot (size, leverage, drawdown), plus the stored ones.
    pub fn account_tags(&self, a: &Account) -> Vec<String> {
        let mut t: Vec<String> = self.tags(&a.address).to_vec();
        let cohort = COHORTS[cohort_of(a.account_value)].0.split(' ').next().unwrap_or("").to_string();
        if !t.contains(&cohort) {
            t.insert(0, cohort);
        }
        if a.leverage() >= 10.0 {
            t.push("High leverage".into());
        }
        if a.account_value > 0.0 && a.upnl() < -0.15 * a.account_value {
            t.push("Underwater".into());
        }
        if a.positions.iter().any(|p| p.liq_distance().is_some_and(|d| d < 0.05)) {
            t.push("Near liquidation".into());
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hyperliquid::{Perf, Position};

    #[test]
    fn precedence_tags_and_persistence() {
        let a = "0x31ca8395cf837de08b24da3f660e77761dfb974b";
        let leader = |addr: &str, av: f64, month: f64, name: Option<&str>| Leader {
            address: addr.into(),
            name: name.map(str::to_string),
            account_value: av,
            month: Perf { pnl: month, ..Default::default() },
            ..Default::default()
        };
        let mut l = Labels::default();
        l.absorb_leaders(&[leader(a, 2e7, 5e6, Some("whale.hl")), leader("0xb", 5e4, -1e4, None)]);
        assert_eq!(l.display(a), "whale.hl");
        assert!(l.tags(a).contains(&"Leviathan".to_string()) && l.tags(a).contains(&"Top 10".to_string()));
        assert!(l.tags(a).contains(&"Top 30d PnL".to_string()));
        l.absorb_vaults(&[Vault {
            name: "HLP".into(),
            address: "0xv".into(),
            leader: a.into(),
            tvl: 1.0,
            apr: 0.1,
            closed: false,
        }]);
        assert_eq!(l.display("0xV"), "Vault: HLP", "case-insensitive");
        assert!(l.tags(a).contains(&"Vault leader".to_string()));
        l.set_user(a, " my fren ");
        assert_eq!(l.display(a), "my fren", "your label wins");
        let acct = Account {
            address: a.into(),
            account_value: 1e5,
            notional: 2e6,
            positions: vec![Position { size: 1.0, value: 100.0, liq_px: Some(97.0), upnl: -2e4, ..Default::default() }],
            ..Default::default()
        };
        let t = l.account_tags(&acct);
        assert!(
            t.contains(&"High leverage".into())
                && t.contains(&"Underwater".into())
                && t.contains(&"Near liquidation".into())
        );
        let tmp = std::env::temp_dir().join(format!("hd-labels-{}.json", std::process::id()));
        l.save(&tmp).unwrap();
        assert_eq!(Labels::load(&tmp).label(a), Some("my fren"));
        l.set_user(a, "");
        assert_eq!(l.display(a), "whale.hl");
        let _ = std::fs::remove_file(tmp);
    }
}
