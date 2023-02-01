use super::{pallet::calculate_spot_price, *};

#[test]
fn capacity_zero_returns_none() {
	let res = calculate_spot_price(
		FixedI64::from(i64::MAX),
		0u32,
		u32::MAX,
		Perbill::from_percent(100),
		Perbill::from_percent(1),
	);
	assert_eq!(res, None)
}

#[test]
fn size_larger_than_capacity_returns_none() {
	let res = calculate_spot_price(
		FixedI64::from(i64::MAX),
		1u32,
		2u32,
		Perbill::from_percent(100),
		Perbill::from_percent(1),
	);
	assert_eq!(res, None)
}

#[test]
fn spot_price_calculation_identity() {
	let res = calculate_spot_price(
		FixedI64::from_float(1.0),
		1000,
		100,
		Perbill::from_percent(10),
		Perbill::from_percent(3),
	);
	assert_eq!(res.unwrap(), FixedI64::from_float(1.0))
}

#[test]
fn spot_price_calculation_u32_max() {
	let res = calculate_spot_price(
		FixedI64::from_float(1.0),
		u32::MAX,
		u32::MAX,
		Perbill::from_percent(100),
		Perbill::from_percent(3),
	);
	assert_eq!(res.unwrap(), FixedI64::from_float(1.0))
}

#[test]
fn spot_price_calculation_u32_traffic_max() {
	let res = calculate_spot_price(
		FixedI64::from(i64::MAX),
		u32::MAX,
		u32::MAX,
		Perbill::from_percent(1),
		Perbill::from_percent(1),
	);
	assert_eq!(res, None)
}

#[test]
fn sustained_target_increases_spot_price() {
	let mut traffic = FixedI64::from_u32(1u32);
	for _ in 0..50 {
		traffic = calculate_spot_price(
			traffic,
			100,
			12,
			Perbill::from_percent(10),
			Perbill::from_percent(100),
		)
		.unwrap();
	}
	assert_eq!(traffic, FixedI64::from_float(2.718103316))
}
