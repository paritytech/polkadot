pub use sc_network::ReputationChange;

/// Unified annoyance cost and good behavior benefits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum UnifiedReputationChange {
	CostMajor(&'static str),
	CostMinor(&'static str),
	CostMajorRepeated(&'static str),
	CostMinorRepeated(&'static str),
	Malicious(&'static str),
	BenefitMinorFirst(&'static str),
	BenefitMinor(&'static str),
	BenefitMajorFirst(&'static str),
	BenefitMajor(&'static str),
}

impl UnifiedReputationChange {
	/// Obtain the cost or benefit associated with
	/// the enum variant.
	///
	/// Order of magnitude rationale:
	///
	/// * the peerset will not connect to a peer whose reputation is below a fixed value
	/// * `max(2% *$rep, 1)` is the delta of convergence towards a reputation of 0
	///
	/// The whole range of an `i32` should be used, so order of magnitude of
	/// something malicious should be `1<<20` (give or take).
	const fn cost_or_benefit(&self) -> i32 {
		match self {
			Self::CostMinor(_) => -100_000,
			Self::CostMajor(_) => -300_000,
			Self::CostMinorRepeated(_) => -200_000,
			Self::CostMajorRepeated(_) => -600_000,
			Self::Malicious(_) => -1_000_000,
			Self::BenefitMajorFirst(_) => 300_000,
			Self::BenefitMajor(_) => 200_000,
			Self::BenefitMinorFirst(_) => 15_000,
			Self::BenefitMinor(_) => 10_000,
		}
	}

	/// Extract the static description.
	pub const fn description(&self) -> &'static str {
		match self {
			Self::CostMinor(description) => description,
			Self::CostMajor(description) => description,
			Self::CostMinorRepeated(description) => description,
			Self::CostMajorRepeated(description) => description,
			Self::Malicious(description) => description,
			Self::BenefitMajorFirst(description) => description,
			Self::BenefitMajor(description) => description,
			Self::BenefitMinorFirst(description) => description,
			Self::BenefitMinor(description) => description,
		}
	}

	/// Convert into a base reputation as used with substrate.
	pub const fn into_base_rep(self) -> ReputationChange {
		ReputationChange::new(
			self.cost_or_benefit(),
			self.description()
		)
	}
}
