mod test_upgrade_from_kusama_dataset;

// Test that an upgrade from previous test environment works.
#[test]
fn test_upgrade_staking_works() {
	let mut storage = sp_runtime::Storage::default();
	for (key, value) in test_upgrade_from_kusama_dataset::KUSAMA.iter() {
		storage.top.insert(key.to_vec(), value.to_vec());
	}
	let mut ext = sp_io::TestExternalities::from(storage);
	ext.execute_with(|| {
		super::Staking::test_do_upgrade();
	});
}
