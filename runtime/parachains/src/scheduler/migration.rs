use super::*;

const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

pub mod v1 {
	use super::*;
	use crate::scheduler_common::CoreAssignment;
	use frame_support::{
		pallet_prelude::ValueQuery, storage_alias, traits::OnRuntimeUpgrade, weights::Weight,
	};

	#[storage_alias]
	pub(super) type Scheduled<T: Config> = StorageValue<Pallet<T>, Vec<CoreAssignment>, ValueQuery>;

	#[derive(Encode, Decode)]
	pub struct QueuedParathread {
		claim: primitives::ParathreadEntry,
		core_offset: u32,
	}

	#[derive(Encode, Decode, Default)]
	pub struct ParathreadClaimQueue {
		queue: Vec<QueuedParathread>,
		next_core_offset: u32,
	}

	#[storage_alias]
	pub(super) type ParathreadQueue<T: Config> =
		StorageValue<Pallet<T>, ParathreadClaimQueue, ValueQuery>;

	#[storage_alias]
	pub(super) type ParathreadClaimIndex<T: Config> =
		StorageValue<Pallet<T>, Vec<ParaId>, ValueQuery>;

	pub struct MigrateToV1<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config> OnRuntimeUpgrade for MigrateToV1<T> {
		fn on_runtime_upgrade() -> Weight {
			let mut weight: Weight = Weight::zero();

			if StorageVersion::get::<Pallet<T>>() < STORAGE_VERSION {
				log::info!(
					target: crate::scheduler::LOG_TARGET,
					"Migrating scheduler storage to v1"
				);
				weight += migrate_to_v1::<T>();
				STORAGE_VERSION.put::<Pallet<T>>();
				weight = weight.saturating_add(T::DbWeight::get().reads_writes(1, 1));
			} else {
				log::info!(
					target: crate::scheduler::LOG_TARGET,
					"Scheduler storage up to date - no need for migration"
				);
			}

			weight
		}

		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<Vec<u8>, &'static str> {
			log::trace!(
				target: crate::scheduler::LOG_TARGET,
				"Scheduled before migration: {}",
				Scheduled::<T>::iter().count()
			);
			ensure!(
				StorageVersion::get::<Pallet<T>>() == 0,
				"Storage version should be less than `1` before the migration",
			);

			Ok(Vec::new())
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(_state: Vec<u8>) -> Result<(), &'static str> {
			log::trace!(target: crate::scheduler::LOG_TARGET, "Running post_upgrade()");
			ensure!(
				StorageVersion::get::<Pallet<T>>() == STORAGE_VERSION,
				"Storage version should be `1` after the migration"
			);
			ensure!(
				Scheduled::<T>::iter().count() == 0,
				"Scheduled should be empty after the migration"
			);

			Ok(())
		}
	}

	pub fn migrate_to_v1<T: crate::scheduler::Config>() -> Weight {
		let mut weight: Weight = Weight::zero();

		let pq = ParathreadQueue::<T>::take();
		let pq_len = pq.queue.len() as u64;

		let pci = ParathreadClaimIndex::<T>::take();
		let pci_len = pci.len() as u64;

		let scheduled = Scheduled::<T>::take();
		let sched_len = scheduled.len() as u64;
		for core_assignment in scheduled {
			Pallet::<T>::add_to_claimqueue(core_assignment.core, core_assignment);
		}

		weight = weight.saturating_add(T::DbWeight::get().reads_writes(sched_len, sched_len));
		// This is for add_to_claimqueue()
		weight = weight.saturating_add(T::DbWeight::get().reads_writes(sched_len, sched_len));
		weight = weight.saturating_add(T::DbWeight::get().reads_writes(pq_len, pq_len));
		weight = weight.saturating_add(T::DbWeight::get().reads_writes(pci_len, pci_len));

		weight
	}
}
