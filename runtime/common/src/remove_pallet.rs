use frame_support::weights::constants::RocksDbWeight;
use sp_core::Get;
use sp_io::hashing::twox_128;
use sp_std::{marker::PhantomData, vec::Vec};

pub struct RemovePallet<P: Get<&'static str>>(PhantomData<P>);
impl<P: Get<&'static str>> RemovePallet<P> {
	fn clear_keys(dry_run: bool) -> (u64, frame_support::weights::Weight) {
		let prefix = twox_128(P::get().as_bytes());
		let mut current = prefix.clone().to_vec();
		let mut counter = 0;
		while let Some(next) = sp_io::storage::next_key(&current[..]) {
			if !next.starts_with(&prefix) {
				break
			}
			if !dry_run {
				sp_io::storage::clear(&next);
			}
			counter += 1;
			current = next;
		}
		(counter, RocksDbWeight::get().reads_writes(counter, counter))
	}
}
impl<P: Get<&'static str>> frame_support::traits::OnRuntimeUpgrade for RemovePallet<P> {
	fn on_runtime_upgrade() -> frame_support::weights::Weight {
		Self::clear_keys(false).1
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<Vec<u8>, &'static str> {
		let (key_count, _) = Self::clear_keys(true);
		if key_count > 0 {
			log::info!("Found {} keys for pallet {} pre-removal", key_count, P::get());
		} else {
			log::warn!("No keys found for pallet {} pre-removal", P::get());
		}
		Ok(Vec::new())
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade(_state: Vec<u8>) -> Result<(), &'static str> {
		let (key_count, _) = Self::clear_keys(true);
		if key_count > 0 {
			log::error!("{} pallet {} keys remaining post-removal ❗", key_count, P::get());
		} else {
			log::info!("No {} keys remaining post-removal 🎉", P::get())
		}
		Ok(())
	}
}
