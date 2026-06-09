use std::time::Instant;

pub(crate) struct StartupTrace {
    enabled: bool,
    label: &'static str,
    start: Instant,
    last: Instant,
}

impl StartupTrace {
    pub(crate) fn new(label: &'static str) -> Self {
        let enabled = startup_log_enabled();
        let now = Instant::now();
        if enabled {
            eprintln!("lk startup: {label}: start");
        }
        Self {
            enabled,
            label,
            start: now,
            last: now,
        }
    }

    pub(crate) fn step(&mut self, name: &'static str) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        eprintln!(
            "lk startup: {}: {} +{:.3}ms total={:.3}ms",
            self.label,
            name,
            now.duration_since(self.last).as_secs_f64() * 1000.0,
            now.duration_since(self.start).as_secs_f64() * 1000.0
        );
        self.last = now;
    }
}

impl Drop for StartupTrace {
    fn drop(&mut self) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        eprintln!(
            "lk startup: {}: done total={:.3}ms",
            self.label,
            now.duration_since(self.start).as_secs_f64() * 1000.0
        );
    }
}

fn startup_log_enabled() -> bool {
    std::env::var("LK_STARTUP_LOG")
        .map(|raw| {
            let trimmed = raw.trim();
            !(trimmed.is_empty()
                || trimmed.eq_ignore_ascii_case("0")
                || trimmed.eq_ignore_ascii_case("false")
                || trimmed.eq_ignore_ascii_case("off"))
        })
        .unwrap_or(false)
}
