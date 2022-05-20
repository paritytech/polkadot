#[derive(Debug, serde::Serialize)]
pub(crate) struct Info {
	pub spec_name: String,
	pub spec_version: u32,
}

impl Info {
	pub fn new(spec_name: String, spec_version: u32) -> Self {
		Self { spec_name, spec_version }
	}
}
