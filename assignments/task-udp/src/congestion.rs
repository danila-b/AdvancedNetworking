const INITIAL_CWND: f64 = 2.0;
const MIN_CWND: f64 = 1.0;
const MAX_CWND: f64 = 30.0;

pub struct CongestionControl {
    cwnd: f64
}

impl CongestionControl {
    pub fn new() -> Self {
        Self {
            cwnd: INITIAL_CWND,
        }
    }

    /// Called when a new ACK is received - we grow the window exponentially up to the maximum window size
    pub fn on_ack(&mut self) {
        self.cwnd += 1.0;
        self.cwnd = self.cwnd.min(MAX_CWND);
    }

    /// On timeout we halve the window
    pub fn on_timeout(&mut self) {
        self.cwnd = (self.cwnd / 2.0).max(MIN_CWND);
    }

    pub fn on_fast_retransmit(&mut self) {
        self.cwnd = (self.cwnd / 2.0).max(MIN_CWND);
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
