const INITIAL_CWND: f64 = 2.0;
const MIN_CWND: f64 = 1.0;
const MAX_CWND: f64 = 64.0;
const INITIAL_SSTHRESH: f64 = 64.0;

pub struct CongestionControl {
    cwnd: f64,
    ssthresh: f64,
}

impl CongestionControl {
    pub fn new() -> Self {
        Self {
            cwnd: INITIAL_CWND,
            ssthresh: INITIAL_SSTHRESH,
        }
    }

    /// Called when a new ACK is received - we grow the window either linearly or exponentially depending on the current window size
    pub fn on_ack(&mut self) {
        if self.cwnd < self.ssthresh {
            self.cwnd += 1.0;
        } else {
            self.cwnd += 1.0 / self.cwnd;
        }
        self.cwnd = self.cwnd.min(MAX_CWND);
    }

    /// On timeout we halve the window and enter slow start
    pub fn on_timeout(&mut self) {
        self.ssthresh = (self.cwnd / 2.0).max(MIN_CWND);
        self.cwnd = MIN_CWND;
    }

    /// On fast retransmit we set the window to the slow start threshold
    pub fn on_fast_retransmit(&mut self) {
        self.ssthresh = (self.cwnd / 2.0).max(MIN_CWND);
        self.cwnd = self.ssthresh;
    }

    pub fn window(&self) -> usize {
        self.cwnd.floor() as usize
    }
}

impl Default for CongestionControl {
    fn default() -> Self {
        Self::new()
    }
}
