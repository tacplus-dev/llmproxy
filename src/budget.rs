//! Per caller × bucket daily budgets and the usage rows behind them (crew
//! runtime §10, backlog §1.2). Spend is keyed by the identity part of the
//! session id (the caller SPIFFE ID); each usage row carries the full session
//! id.
//!
//! In memory, today only: `LlmProxyState` declares no usage record, so a
//! restart resets the day's spend (recorded in llmproxy-impl.questions.md).
//! The day is the UTC day until tenant-local midnight (A18) has a source.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use llmproxy_model::types::services::llmproxy::llmproxy::Config as LlmProxyConfig;
use llmproxy_model::types::services::llmproxy::llmproxy::BudgetBucket;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    pub session_id: String,
    pub bucket: BudgetBucket,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_micro_usd: i64,
}

#[derive(Default)]
pub struct Ledger {
    day: i64,
    spent: HashMap<(String, BudgetBucket), i64>,
    rows: Vec<UsageRow>,
}

impl Ledger {
    pub fn limit(cfg: &LlmProxyConfig, bucket: BudgetBucket) -> i64 {
        match bucket {
            BudgetBucket::DayJob => cfg.default_day_job_micro_usd,
            BudgetBucket::AboveAndBeyond => cfg.default_above_and_beyond_micro_usd,
            BudgetBucket::Reserve => cfg.default_reserve_micro_usd,
            BudgetBucket::Personal => cfg.personal_micro_usd,
            BudgetBucket::Unspecified => 0,
        }
    }

    pub fn spent(&mut self, caller: &str, bucket: BudgetBucket) -> i64 {
        self.roll_day();
        self.spent.get(&(caller.to_string(), bucket)).copied().unwrap_or(0)
    }

    pub fn charge(&mut self, caller: &str, row: UsageRow) {
        self.roll_day();
        *self.spent.entry((caller.to_string(), row.bucket)).or_insert(0) += row.cost_micro_usd;
        tracing::info!(
            session_id = %row.session_id,
            bucket = row.bucket.as_str_name(),
            input_tokens = row.input_tokens,
            output_tokens = row.output_tokens,
            cost_micro_usd = row.cost_micro_usd,
            "usage"
        );
        self.rows.push(row);
    }

    /// Today's usage rows.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[UsageRow] {
        &self.rows
    }

    /// When today's spend resets (epoch ms).
    pub fn resets_at_ms() -> i64 {
        (today() + 1) * DAY_MS
    }

    fn roll_day(&mut self) {
        let day = today();
        if day != self.day {
            self.day = day;
            self.spent.clear();
            self.rows.clear();
        }
    }
}

fn today() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64 / DAY_MS
}
