use prometheus;
#[derive(Clone)]
struct MetricsInner {
	gossipped_own_availability_bitfields: prometheus::Counter<prometheus::U64>,
	received_availability_bitfields: prometheus::Counter<prometheus::U64>,
}

/// Bitfield Distribution metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_own_bitfield_gossipped(&self) {
		if let Some(metrics) = &self.0 {
			metrics.gossipped_own_availability_bitfields.inc();
		}
	}

	fn on_bitfield_received(&self) {
		if let Some(metrics) = &self.0 {
			metrics.received_availability_bitfields.inc();
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			gossipped_own_availability_bitfields: prometheus::register(
				prometheus::Counter::new(
					"parachain_gossipped_own_availabilty_bitfields_total",
					"Number of own availability bitfields sent to other peers."
				)?,
				registry,
			)?,
			received_availability_bitfields: prometheus::register(
				prometheus::Counter::new(
					"parachain_received_availabilty_bitfields_total",
					"Number of valid availability bitfields received from other peers."
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
