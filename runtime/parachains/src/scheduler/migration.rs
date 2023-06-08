use super::*;

const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

pub mod v1 {
	use super::*;
	use frame_support::{
		pallet_prelude::ValueQuery, storage_alias, traits::OnRuntimeUpgrade, weights::Weight,
	};
	use primitives::{v4::CollatorRestrictions, vstaging::Assignment, CollatorId};

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

	/// The assignment type.
	#[derive(Clone, Encode, Decode, TypeInfo)]
	#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
	pub enum AssignmentKind {
		/// A parachain.
		Parachain,
		/// A parathread.
		Parathread(CollatorId, u32),
	}

	/// How a free core is scheduled to be assigned.
	#[derive(Clone, Encode, Decode, TypeInfo)]
	#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
	pub struct CoreAssignment {
		/// The core that is assigned.
		pub core: CoreIndex,
		/// The unique ID of the para that is assigned to the core.
		pub para_id: ParaId,
		/// The kind of the assignment.
		pub kind: AssignmentKind,
		/// The index of the validator group assigned to the core.
		pub group_idx: GroupIndex,
	}

	impl CoreAssignment {
		/// Get the ID of a collator who is required to collate this block.
		pub fn required_collator(self) -> Option<CollatorId> {
			match self.kind {
				AssignmentKind::Parachain => None,
				AssignmentKind::Parathread(id, _) => Some(id),
			}
		}
	}

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
		fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::DispatchError> {
			log::trace!(
				target: crate::scheduler::LOG_TARGET,
				"Scheduled before migration: {}",
				Scheduled::<T>::get().len()
			);
			ensure!(
				StorageVersion::get::<Pallet<T>>() == 0,
				"Storage version should be less than `1` before the migration",
			);

			let bytes = u32::to_be_bytes(Scheduled::<T>::get().len() as u32);

			Ok(bytes.to_vec())
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::DispatchError> {
			log::trace!(target: crate::scheduler::LOG_TARGET, "Running post_upgrade()");
			ensure!(
				StorageVersion::get::<Pallet<T>>() == STORAGE_VERSION,
				"Storage version should be `1` after the migration"
			);
			ensure!(
				Scheduled::<T>::get().len() == 0,
				"Scheduled should be empty after the migration"
			);

			let sched_len = u32::from_be_bytes(state.try_into().unwrap());
			ensure!(
				Pallet::<T>::claimqueue_len() as u32 == sched_len,
				"Scheduled completely moved to ClaimQueue after migration"
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
			let core_idx = core_assignment.core;
			let assignment = Assignment::new(core_assignment.para_id, CollatorRestrictions::none());
			let pe = ParasEntry::new(assignment);
			Pallet::<T>::add_to_claimqueue(core_idx, pe);
		}

		// 2x as once for Scheduled and once for Claimqueue
		weight =
			weight.saturating_add(T::DbWeight::get().reads_writes(2 * sched_len, 2 * sched_len));
		weight = weight.saturating_add(T::DbWeight::get().reads_writes(pq_len, pq_len));
		weight = weight.saturating_add(T::DbWeight::get().reads_writes(pci_len, pci_len));

		weight
	}
}
