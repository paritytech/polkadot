use super::{pallet::calculate_spot_price, *};

#[test]
fn spot_price_calculation_identity() {
	let traffic = FixedI64::from_float(1.0);
	let res = calculate_spot_price(
		traffic,
		1000,
		100,
		Perbill::from_percent(10),
		Perbill::from_percent(3),
	);
	assert_eq!(res.unwrap(), FixedI64::from_float(1.0))
}

#[test]
fn spot_price_calculation_u32_max() {
	let traffic = FixedI64::from_float(1.0);
	let res = calculate_spot_price(
		traffic,
		u32::MAX,
		u32::MAX,
		Perbill::from_percent(100),
		Perbill::from_percent(3),
	);
	assert_eq!(res.unwrap(), FixedI64::from_float(1.0))
}

#[test]
fn spot_price_calculation_u32_traffic_max() {
	let traffic = FixedI64::from(i64::MAX);
	let res = calculate_spot_price(
		traffic,
		u32::MAX,
		u32::MAX,
		Perbill::from_percent(1),
		Perbill::from_percent(1),
	);
	assert_eq!(res, None)
}
