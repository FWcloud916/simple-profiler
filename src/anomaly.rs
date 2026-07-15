use std::collections::HashMap;

use crate::{
    config::{AnomalyConfig, AnomalyRuleConfig},
    model::{Metric, MetricBatch},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "warning" => Some(Self::Warning),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Normal,
    Pending,
    Open,
    Recovering,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Pending => "pending",
            Self::Open => "open",
            Self::Recovering => "recovering",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "normal" => Some(Self::Normal),
            "pending" => Some(Self::Pending),
            "open" => Some(Self::Open),
            "recovering" => Some(Self::Recovering),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StateKey {
    pub rule_id: String,
    pub resource: String,
}

#[derive(Debug, Clone)]
pub struct RuleState {
    pub phase: Phase,
    pub severity: Option<Severity>,
    pub pending_severity: Option<Severity>,
    pub pending_since_ms: Option<i64>,
    pub pending_samples: u64,
    pub critical_since_ms: Option<i64>,
    pub critical_samples: u64,
    pub recovery_since_ms: Option<i64>,
    pub recovery_samples: u64,
    pub event_id: Option<i64>,
    pub last_sample_ms: Option<i64>,
    pub last_value: Option<f64>,
    pub peak_value: Option<f64>,
    pub peak_at_ms: Option<i64>,
    pub last_evidence_ms: Option<i64>,
    pub data_gap_count: u64,
}

impl Default for RuleState {
    fn default() -> Self {
        Self {
            phase: Phase::Normal,
            severity: None,
            pending_severity: None,
            pending_since_ms: None,
            pending_samples: 0,
            critical_since_ms: None,
            critical_samples: 0,
            recovery_since_ms: None,
            recovery_samples: 0,
            event_id: None,
            last_sample_ms: None,
            last_value: None,
            peak_value: None,
            peak_at_ms: None,
            last_evidence_ms: None,
            data_gap_count: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    None,
    Open,
    Escalate,
    Close,
}

#[derive(Debug, Clone)]
pub struct Evaluation {
    pub key: StateKey,
    pub rule: AnomalyRuleConfig,
    pub metric: Metric,
    pub transition: Transition,
    pub event_id: Option<i64>,
    pub event_started_at_ms: Option<i64>,
    pub trigger_sample_count: u64,
    pub new_peak: bool,
    pub periodic_evidence: bool,
    pub data_gap: bool,
    pub state: RuleState,
}

#[derive(Debug, Clone)]
pub struct AnomalyEngine {
    enabled: bool,
    evidence_interval_ms: i64,
    rules: Vec<AnomalyRuleConfig>,
    states: HashMap<StateKey, RuleState>,
}

impl AnomalyEngine {
    pub fn new(config: &AnomalyConfig) -> Self {
        Self {
            enabled: config.enabled,
            evidence_interval_ms: seconds_ms(config.evidence_interval_seconds),
            rules: config.rules.clone(),
            states: HashMap::new(),
        }
    }

    pub fn restore(&mut self, key: StateKey, state: RuleState) {
        if self.rules.iter().any(|rule| rule.id == key.rule_id) {
            self.states.insert(key, state);
        }
    }

    pub fn states(&self) -> &HashMap<StateKey, RuleState> {
        &self.states
    }

    pub fn state(&self, key: &StateKey) -> Option<&RuleState> {
        self.states.get(key)
    }

    pub fn attach_event(&mut self, key: &StateKey, event_id: i64, detected_at_ms: i64) {
        if let Some(state) = self.states.get_mut(key) {
            state.event_id = Some(event_id);
            state.last_evidence_ms = Some(detected_at_ms);
        }
    }

    pub fn evaluate_batch(&mut self, batch: &MetricBatch) -> Vec<Evaluation> {
        if !self.enabled {
            return Vec::new();
        }
        let mut evaluations = Vec::new();
        for metric in batch {
            if !metric.value.is_finite() {
                continue;
            }
            let matching_rules: Vec<_> = self
                .rules
                .iter()
                .filter(|rule| rule.enabled && rule.metric_name == metric.name)
                .cloned()
                .collect();
            for rule in matching_rules {
                if let Some(evaluation) = self.evaluate_metric(&rule, metric) {
                    evaluations.push(evaluation);
                }
            }
        }
        evaluations
    }

    fn evaluate_metric(&mut self, rule: &AnomalyRuleConfig, metric: &Metric) -> Option<Evaluation> {
        let timestamp_ms = metric.collected_at.timestamp_millis();
        let key = StateKey {
            rule_id: rule.id.clone(),
            resource: metric.resource.clone().unwrap_or_default(),
        };
        let state = self.states.entry(key.clone()).or_default();
        if state
            .last_sample_ms
            .is_some_and(|last_sample| timestamp_ms <= last_sample)
        {
            return None;
        }

        let data_gap = state.last_sample_ms.is_some_and(|last_sample| {
            timestamp_ms.saturating_sub(last_sample) > seconds_ms(rule.max_sample_gap_seconds)
        });
        if data_gap {
            if state.phase == Phase::Pending {
                reset_to_normal(state);
            } else if matches!(state.phase, Phase::Open | Phase::Recovering) {
                state.data_gap_count = state.data_gap_count.saturating_add(1);
                clear_critical_candidate(state);
                clear_recovery_candidate(state);
                state.phase = Phase::Open;
            }
        }

        state.last_sample_ms = Some(timestamp_ms);
        state.last_value = Some(metric.value);
        let old_peak = state.peak_value;
        if matches!(
            state.phase,
            Phase::Pending | Phase::Open | Phase::Recovering
        ) && old_peak.is_none_or(|peak| metric.value > peak)
        {
            state.peak_value = Some(metric.value);
            state.peak_at_ms = Some(timestamp_ms);
        }

        let event_id_before = state.event_id;
        let mut event_started_at_ms = None;
        let mut trigger_sample_count = 0;
        let transition = match state.phase {
            Phase::Normal => begin_pending_or_stay_normal(state, rule, metric.value, timestamp_ms),
            Phase::Pending => {
                let result = continue_pending(state, rule, metric.value, timestamp_ms);
                if result == Transition::Open {
                    event_started_at_ms = state.pending_since_ms;
                    trigger_sample_count = state.pending_samples;
                    state.phase = Phase::Open;
                    state.severity = state.pending_severity;
                    clear_pending(state);
                }
                result
            }
            Phase::Open => continue_open(state, rule, metric.value, timestamp_ms),
            Phase::Recovering => continue_recovering(state, rule, metric.value, timestamp_ms),
        };

        if state.phase == Phase::Pending && candidate_matured(state, rule, timestamp_ms) {
            event_started_at_ms = state.pending_since_ms;
            trigger_sample_count = state.pending_samples;
            state.phase = Phase::Open;
            state.severity = state.pending_severity;
            clear_pending(state);
            return Some(Evaluation {
                key,
                rule: rule.clone(),
                metric: metric.clone(),
                transition: Transition::Open,
                event_id: event_id_before,
                event_started_at_ms,
                trigger_sample_count,
                new_peak: true,
                periodic_evidence: false,
                data_gap,
                state: state.clone(),
            });
        }

        let new_peak =
            event_id_before.is_some() && state.peak_value.is_some() && state.peak_value != old_peak;
        let periodic_evidence = event_id_before.is_some()
            && transition != Transition::Close
            && state
                .last_evidence_ms
                .is_none_or(|last| timestamp_ms.saturating_sub(last) >= self.evidence_interval_ms);
        if periodic_evidence {
            state.last_evidence_ms = Some(timestamp_ms);
        }
        if transition == Transition::Close {
            let closed_event = event_id_before;
            let closed_state = state.clone();
            reset_to_normal(state);
            return Some(Evaluation {
                key,
                rule: rule.clone(),
                metric: metric.clone(),
                transition,
                event_id: closed_event,
                event_started_at_ms,
                trigger_sample_count,
                new_peak,
                periodic_evidence: false,
                data_gap,
                state: closed_state,
            });
        }

        Some(Evaluation {
            key,
            rule: rule.clone(),
            metric: metric.clone(),
            transition,
            event_id: state.event_id,
            event_started_at_ms,
            trigger_sample_count,
            new_peak,
            periodic_evidence,
            data_gap,
            state: state.clone(),
        })
    }
}

fn begin_pending_or_stay_normal(
    state: &mut RuleState,
    rule: &AnomalyRuleConfig,
    value: f64,
    timestamp_ms: i64,
) -> Transition {
    if let Some(severity) = breached_severity(rule, value) {
        state.phase = Phase::Pending;
        state.pending_severity = Some(severity);
        state.pending_since_ms = Some(timestamp_ms);
        state.pending_samples = 1;
        state.peak_value = Some(value);
        state.peak_at_ms = Some(timestamp_ms);
    }
    Transition::None
}

fn continue_pending(
    state: &mut RuleState,
    rule: &AnomalyRuleConfig,
    value: f64,
    timestamp_ms: i64,
) -> Transition {
    let Some(severity) = breached_severity(rule, value) else {
        reset_to_normal(state);
        return Transition::None;
    };
    if state.pending_severity != Some(severity) {
        state.pending_severity = Some(severity);
        state.pending_since_ms = Some(timestamp_ms);
        state.pending_samples = 1;
    } else {
        state.pending_samples = state.pending_samples.saturating_add(1);
    }
    if candidate_matured(state, rule, timestamp_ms) {
        Transition::Open
    } else {
        Transition::None
    }
}

fn continue_open(
    state: &mut RuleState,
    rule: &AnomalyRuleConfig,
    value: f64,
    timestamp_ms: i64,
) -> Transition {
    if value <= rule.recovery_threshold {
        state.phase = Phase::Recovering;
        state.recovery_since_ms = Some(timestamp_ms);
        state.recovery_samples = 1;
        return Transition::None;
    }
    clear_recovery_candidate(state);
    if state.severity == Some(Severity::Warning) && value >= rule.critical_threshold {
        if state.critical_since_ms.is_none() {
            state.critical_since_ms = Some(timestamp_ms);
            state.critical_samples = 1;
        } else {
            state.critical_samples = state.critical_samples.saturating_add(1);
        }
        if matured(
            state.critical_since_ms,
            state.critical_samples,
            rule.critical_trigger_seconds,
            rule.critical_min_samples,
            timestamp_ms,
        ) {
            state.severity = Some(Severity::Critical);
            clear_critical_candidate(state);
            return Transition::Escalate;
        }
    } else {
        clear_critical_candidate(state);
    }
    Transition::None
}

fn continue_recovering(
    state: &mut RuleState,
    rule: &AnomalyRuleConfig,
    value: f64,
    timestamp_ms: i64,
) -> Transition {
    if value >= rule.warning_threshold {
        state.phase = Phase::Open;
        clear_recovery_candidate(state);
        return continue_open(state, rule, value, timestamp_ms);
    }
    if value <= rule.recovery_threshold {
        state.recovery_samples = state.recovery_samples.saturating_add(1);
        if matured(
            state.recovery_since_ms,
            state.recovery_samples,
            rule.recovery_seconds,
            rule.recovery_min_samples,
            timestamp_ms,
        ) {
            return Transition::Close;
        }
    } else {
        state.phase = Phase::Open;
        clear_recovery_candidate(state);
    }
    Transition::None
}

fn breached_severity(rule: &AnomalyRuleConfig, value: f64) -> Option<Severity> {
    if value >= rule.critical_threshold {
        Some(Severity::Critical)
    } else if value >= rule.warning_threshold {
        Some(Severity::Warning)
    } else {
        None
    }
}

fn candidate_matured(state: &RuleState, rule: &AnomalyRuleConfig, timestamp_ms: i64) -> bool {
    match state.pending_severity {
        Some(Severity::Warning) => matured(
            state.pending_since_ms,
            state.pending_samples,
            rule.trigger_seconds,
            rule.min_samples,
            timestamp_ms,
        ),
        Some(Severity::Critical) => matured(
            state.pending_since_ms,
            state.pending_samples,
            rule.critical_trigger_seconds,
            rule.critical_min_samples,
            timestamp_ms,
        ),
        None => false,
    }
}

fn matured(
    since_ms: Option<i64>,
    samples: u64,
    duration_seconds: u64,
    min_samples: u64,
    timestamp_ms: i64,
) -> bool {
    samples >= min_samples
        && since_ms
            .is_some_and(|since| timestamp_ms.saturating_sub(since) >= seconds_ms(duration_seconds))
}

fn clear_pending(state: &mut RuleState) {
    state.pending_severity = None;
    state.pending_since_ms = None;
    state.pending_samples = 0;
}

fn clear_critical_candidate(state: &mut RuleState) {
    state.critical_since_ms = None;
    state.critical_samples = 0;
}

fn clear_recovery_candidate(state: &mut RuleState) {
    state.recovery_since_ms = None;
    state.recovery_samples = 0;
}

fn reset_to_normal(state: &mut RuleState) {
    *state = RuleState {
        last_sample_ms: state.last_sample_ms,
        last_value: state.last_value,
        ..RuleState::default()
    };
}

fn seconds_ms(seconds: u64) -> i64 {
    i64::try_from(seconds)
        .unwrap_or(i64::MAX / 1_000)
        .saturating_mul(1_000)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn metric(at_seconds: i64, value: f64) -> Metric {
        Metric::new(
            Utc.timestamp_opt(at_seconds, 0).single().expect("time"),
            "system",
            "cpu.total.usage",
            value,
            "percent",
        )
    }

    fn test_config() -> AnomalyConfig {
        AnomalyConfig {
            evidence_interval_seconds: 10,
            rules: vec![AnomalyRuleConfig {
                id: "cpu".to_owned(),
                metric_name: "cpu.total.usage".to_owned(),
                trigger_seconds: 10,
                critical_trigger_seconds: 5,
                recovery_seconds: 10,
                min_samples: 3,
                critical_min_samples: 2,
                recovery_min_samples: 3,
                max_sample_gap_seconds: 6,
                ..AnomalyRuleConfig::default()
            }],
            ..AnomalyConfig::default()
        }
    }

    #[test]
    fn sustained_warning_opens_and_hysteresis_closes_it() {
        let mut engine = AnomalyEngine::new(&test_config());
        let first = engine.evaluate_batch(&vec![metric(0, 91.0)]);
        let second = engine.evaluate_batch(&vec![metric(5, 92.0)]);
        let third = engine.evaluate_batch(&vec![metric(10, 93.0)]);

        assert_eq!(first[0].transition, Transition::None);
        assert_eq!(second[0].transition, Transition::None);
        assert_eq!(third[0].transition, Transition::Open);
        engine.attach_event(&third[0].key, 7, 10_000);

        assert_eq!(
            engine.evaluate_batch(&vec![metric(15, 80.0)])[0].transition,
            Transition::None
        );
        assert_eq!(
            engine.evaluate_batch(&vec![metric(20, 74.0)])[0].transition,
            Transition::None
        );
        assert_eq!(
            engine.evaluate_batch(&vec![metric(25, 74.0)])[0].transition,
            Transition::None
        );
        assert_eq!(
            engine.evaluate_batch(&vec![metric(30, 74.0)])[0].transition,
            Transition::Close
        );
    }

    #[test]
    fn a_data_gap_resets_pending_detection() {
        let mut engine = AnomalyEngine::new(&test_config());
        engine.evaluate_batch(&vec![metric(0, 91.0)]);
        engine.evaluate_batch(&vec![metric(5, 92.0)]);

        let after_gap = engine.evaluate_batch(&vec![metric(30, 93.0)]);

        assert_eq!(after_gap[0].transition, Transition::None);
        assert_eq!(
            engine
                .state(&after_gap[0].key)
                .expect("state")
                .pending_samples,
            1
        );
    }

    #[test]
    fn duplicate_or_older_samples_are_ignored() {
        let mut engine = AnomalyEngine::new(&test_config());
        engine.evaluate_batch(&vec![metric(5, 91.0)]);

        assert!(engine.evaluate_batch(&vec![metric(5, 92.0)]).is_empty());
        assert!(engine.evaluate_batch(&vec![metric(4, 92.0)]).is_empty());
    }

    #[test]
    fn a_short_spike_does_not_open_an_event() {
        let mut engine = AnomalyEngine::new(&test_config());
        engine.evaluate_batch(&vec![metric(0, 99.0)]);
        let normal = engine.evaluate_batch(&vec![metric(5, 50.0)]);

        assert_eq!(normal[0].transition, Transition::None);
        assert_eq!(
            engine.state(&normal[0].key).expect("state").phase,
            Phase::Normal
        );
    }

    #[test]
    fn an_open_warning_escalates_after_sustained_critical_samples() {
        let mut engine = AnomalyEngine::new(&test_config());
        engine.evaluate_batch(&vec![metric(0, 91.0)]);
        engine.evaluate_batch(&vec![metric(5, 92.0)]);
        let opened = engine.evaluate_batch(&vec![metric(10, 93.0)]);
        engine.attach_event(&opened[0].key, 8, 10_000);

        assert_eq!(
            engine.evaluate_batch(&vec![metric(15, 98.0)])[0].transition,
            Transition::None
        );
        let escalated = engine.evaluate_batch(&vec![metric(20, 99.0)]);

        assert_eq!(escalated[0].transition, Transition::Escalate);
        assert_eq!(escalated[0].state.severity, Some(Severity::Critical));
    }
}
