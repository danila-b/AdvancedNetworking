use std::time::Duration;

const INITIAL_RTO_MS: f64 = 1000.0;
const MIN_RTO_MS: f64 = 200.0;
const MAX_RTO_MS: f64 = 10000.0;

pub struct RttEstimator {
    srtt: f64,
    rttvar: f64,
    pub rto: Duration,
}

impl RttEstimator {
    pub fn new() -> Self {
        Self {
            srtt: 0.0,
            rttvar: 0.0,
            rto: Duration::from_millis(INITIAL_RTO_MS as u64),
        }
    }

    pub fn update(&mut self, rtt_ms: f64) {
        if self.srtt == 0.0 {
            // First measurement
            self.srtt = rtt_ms;
            self.rttvar = rtt_ms / 2.0;
        } else {
            // These formulas are shamelessly stolen from Jacobson/Karels algorithm: https://tcpcc.systemsapproach.org/algorithm.html
            self.rttvar = 0.75 * self.rttvar + 0.25 * (self.srtt - rtt_ms).abs();
            self.srtt = 0.875 * self.srtt + 0.125 * rtt_ms;
        }
        self.update_rto();
    }

    /// Double RTO on timeout - idea also shamelessly stolen from Karn's algorithm https://tcpcc.systemsapproach.org/algorithm.html
    pub fn backoff(&mut self) {
        let new_rto_ms = (self.rto.as_millis() as f64 * 2.0).min(MAX_RTO_MS);
        self.rto = Duration::from_millis(new_rto_ms as u64);
    }

    fn update_rto(&mut self) {
        let rto_ms = (self.srtt + 4.0 * self.rttvar).clamp(MIN_RTO_MS, MAX_RTO_MS);
        self.rto = Duration::from_millis(rto_ms as u64);
    }
}

impl Default for RttEstimator {
    fn default() -> Self {
        Self::new()
    }
}
