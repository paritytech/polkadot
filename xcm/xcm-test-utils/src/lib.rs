use xcm::opaque::v0::opaque::Xcm;
use xcm::opaque::v0::prelude::*;
use sp_std::cell::RefCell;
use sp_std::prelude::*;

#[cfg(test)]
mod kusama_tests;

// #[cfg(test)]
// mod tests;

thread_local! {
	pub static SENT_XCM: RefCell<Vec<(MultiLocation, Xcm)>> = RefCell::new(Vec::new());
}
pub fn sent_xcm() -> Vec<(MultiLocation, Xcm)> {
	SENT_XCM.with(|q| (*q.borrow()).clone())
}
pub struct MockXcmSender;
impl SendXcm for MockXcmSender {
	fn send_xcm(dest: MultiLocation, msg: Xcm) -> XcmResult {
		SENT_XCM.with(|q| q.borrow_mut().push((dest, msg)));
		Ok(())
	}
}
