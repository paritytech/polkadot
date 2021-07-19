use kusama_runtime::{Block, Runtime};

#[tokio::test]
async fn test_voter_bags_migration() {
	crate::helpers::test_voter_bags_migration::<Runtime, Block>().await;
}
