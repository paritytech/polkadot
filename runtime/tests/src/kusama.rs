use kusama_runtime::{Block, Runtime, UNITS};

#[tokio::test]
async fn test_voter_bags_migration() {
	crate::helpers::test_voter_bags_migration::<Runtime, Block>(UNITS as u64).await;
}
