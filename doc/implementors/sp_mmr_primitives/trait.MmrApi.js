(function() {var implementors = {
"kusama_runtime":[["impl&lt;__SrApiBlock__: BlockT + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>, RuntimeApiImplCall: CallApiAt&lt;__SrApiBlock__&gt; + 'static&gt; MmrApi&lt;__SrApiBlock__, H256, <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.u32.html\">u32</a>&gt; for <a class=\"struct\" href=\"kusama_runtime/struct.RuntimeApiImpl.html\" title=\"struct kusama_runtime::RuntimeApiImpl\">RuntimeApiImpl</a>&lt;__SrApiBlock__, RuntimeApiImplCall&gt;<span class=\"where fmt-newline\">where\n    RuntimeApiImplCall::StateBackend: StateBackend&lt;HashingFor&lt;__SrApiBlock__&gt;&gt;,\n    <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.reference.html\">&amp;'static RuntimeApiImplCall</a>: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/marker/trait.Send.html\" title=\"trait core::marker::Send\">Send</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;&lt;&lt;<a class=\"struct\" href=\"kusama_runtime/struct.Runtime.html\" title=\"struct kusama_runtime::Runtime\">Runtime</a> as Config&gt;::Hashing as Hash&gt;::Output, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;LeafIndex, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.BlockNumber.html\" title=\"type polkadot_core_primitives::BlockNumber\">BlockNumber</a>&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/option/enum.Option.html\" title=\"enum core::option::Option\">Option</a>&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.BlockNumber.html\" title=\"type polkadot_core_primitives::BlockNumber\">BlockNumber</a>&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;(<a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;EncodableOpaqueLeaf&gt;, Proof&lt;&lt;&lt;<a class=\"struct\" href=\"kusama_runtime/struct.Runtime.html\" title=\"struct kusama_runtime::Runtime\">Runtime</a> as Config&gt;::Hashing as Hash&gt;::Output&gt;), Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;EncodableOpaqueLeaf&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    Proof&lt;&lt;&lt;<a class=\"struct\" href=\"kusama_runtime/struct.Runtime.html\" title=\"struct kusama_runtime::Runtime\">Runtime</a> as Config&gt;::Hashing as Hash&gt;::Output&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.unit.html\">()</a>, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    &lt;&lt;<a class=\"struct\" href=\"kusama_runtime/struct.Runtime.html\" title=\"struct kusama_runtime::Runtime\">Runtime</a> as Config&gt;::Hashing as Hash&gt;::Output: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    __SrApiBlock__::Header: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,</span>"]],
"polkadot_runtime":[["impl&lt;__SrApiBlock__: BlockT + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>, RuntimeApiImplCall: CallApiAt&lt;__SrApiBlock__&gt; + 'static&gt; MmrApi&lt;__SrApiBlock__, H256, <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.u32.html\">u32</a>&gt; for <a class=\"struct\" href=\"polkadot_runtime/struct.RuntimeApiImpl.html\" title=\"struct polkadot_runtime::RuntimeApiImpl\">RuntimeApiImpl</a>&lt;__SrApiBlock__, RuntimeApiImplCall&gt;<span class=\"where fmt-newline\">where\n    RuntimeApiImplCall::StateBackend: StateBackend&lt;HashingFor&lt;__SrApiBlock__&gt;&gt;,\n    <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.reference.html\">&amp;'static RuntimeApiImplCall</a>: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/marker/trait.Send.html\" title=\"trait core::marker::Send\">Send</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.Hash.html\" title=\"type polkadot_core_primitives::Hash\">Hash</a>, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;LeafIndex, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.BlockNumber.html\" title=\"type polkadot_core_primitives::BlockNumber\">BlockNumber</a>&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/option/enum.Option.html\" title=\"enum core::option::Option\">Option</a>&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.BlockNumber.html\" title=\"type polkadot_core_primitives::BlockNumber\">BlockNumber</a>&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;(<a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;EncodableOpaqueLeaf&gt;, Proof&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.Hash.html\" title=\"type polkadot_core_primitives::Hash\">Hash</a>&gt;), Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;EncodableOpaqueLeaf&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    Proof&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.Hash.html\" title=\"type polkadot_core_primitives::Hash\">Hash</a>&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.unit.html\">()</a>, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"type\" href=\"polkadot_core_primitives/type.Hash.html\" title=\"type polkadot_core_primitives::Hash\">Hash</a>: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    __SrApiBlock__::Header: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,</span>"]],
"polkadot_test_runtime":[["impl&lt;__SrApiBlock__: BlockT + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>, RuntimeApiImplCall: CallApiAt&lt;__SrApiBlock__&gt; + 'static&gt; MmrApi&lt;__SrApiBlock__, &lt;Block&lt;Header&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.u32.html\">u32</a>, BlakeTwo256&gt;, UncheckedExtrinsic&lt;MultiAddress&lt;&lt;&lt;MultiSignature as Verify&gt;::Signer as IdentifyAccount&gt;::AccountId, <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.u32.html\">u32</a>&gt;, <a class=\"enum\" href=\"polkadot_test_runtime/enum.RuntimeCall.html\" title=\"enum polkadot_test_runtime::RuntimeCall\">RuntimeCall</a>, MultiSignature, (CheckNonZeroSender&lt;<a class=\"struct\" href=\"polkadot_test_runtime/struct.Runtime.html\" title=\"struct polkadot_test_runtime::Runtime\">Runtime</a>&gt;, CheckSpecVersion&lt;<a class=\"struct\" href=\"polkadot_test_runtime/struct.Runtime.html\" title=\"struct polkadot_test_runtime::Runtime\">Runtime</a>&gt;, CheckTxVersion&lt;<a class=\"struct\" href=\"polkadot_test_runtime/struct.Runtime.html\" title=\"struct polkadot_test_runtime::Runtime\">Runtime</a>&gt;, CheckGenesis&lt;<a class=\"struct\" href=\"polkadot_test_runtime/struct.Runtime.html\" title=\"struct polkadot_test_runtime::Runtime\">Runtime</a>&gt;, CheckMortality&lt;<a class=\"struct\" href=\"polkadot_test_runtime/struct.Runtime.html\" title=\"struct polkadot_test_runtime::Runtime\">Runtime</a>&gt;, CheckNonce&lt;<a class=\"struct\" href=\"polkadot_test_runtime/struct.Runtime.html\" title=\"struct polkadot_test_runtime::Runtime\">Runtime</a>&gt;, CheckWeight&lt;<a class=\"struct\" href=\"polkadot_test_runtime/struct.Runtime.html\" title=\"struct polkadot_test_runtime::Runtime\">Runtime</a>&gt;, ChargeTransactionPayment&lt;<a class=\"struct\" href=\"polkadot_test_runtime/struct.Runtime.html\" title=\"struct polkadot_test_runtime::Runtime\">Runtime</a>&gt;)&gt;&gt; as Block&gt;::Hash, <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.u32.html\">u32</a>&gt; for <a class=\"struct\" href=\"polkadot_test_runtime/struct.RuntimeApiImpl.html\" title=\"struct polkadot_test_runtime::RuntimeApiImpl\">RuntimeApiImpl</a>&lt;__SrApiBlock__, RuntimeApiImplCall&gt;<span class=\"where fmt-newline\">where\n    RuntimeApiImplCall::StateBackend: StateBackend&lt;HashingFor&lt;__SrApiBlock__&gt;&gt;,\n    <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.reference.html\">&amp;'static RuntimeApiImplCall</a>: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/marker/trait.Send.html\" title=\"trait core::marker::Send\">Send</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;<a class=\"type\" href=\"polkadot_test_runtime/type.Hash.html\" title=\"type polkadot_test_runtime::Hash\">Hash</a>, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;LeafIndex, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.BlockNumber.html\" title=\"type polkadot_core_primitives::BlockNumber\">BlockNumber</a>&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/option/enum.Option.html\" title=\"enum core::option::Option\">Option</a>&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.BlockNumber.html\" title=\"type polkadot_core_primitives::BlockNumber\">BlockNumber</a>&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;(<a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;EncodableOpaqueLeaf&gt;, Proof&lt;<a class=\"type\" href=\"polkadot_test_runtime/type.Hash.html\" title=\"type polkadot_test_runtime::Hash\">Hash</a>&gt;), Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;EncodableOpaqueLeaf&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    Proof&lt;<a class=\"type\" href=\"polkadot_test_runtime/type.Hash.html\" title=\"type polkadot_test_runtime::Hash\">Hash</a>&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.unit.html\">()</a>, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"type\" href=\"polkadot_test_runtime/type.Hash.html\" title=\"type polkadot_test_runtime::Hash\">Hash</a>: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    __SrApiBlock__::Header: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,</span>"]],
"rococo_runtime":[["impl&lt;__SrApiBlock__: BlockT + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>, RuntimeApiImplCall: CallApiAt&lt;__SrApiBlock__&gt; + 'static&gt; MmrApi&lt;__SrApiBlock__, &lt;&lt;<a class=\"struct\" href=\"rococo_runtime/struct.Runtime.html\" title=\"struct rococo_runtime::Runtime\">Runtime</a> as Config&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.unit.html\">()</a>&gt;&gt;::Hashing as Hash&gt;::Output, <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.u32.html\">u32</a>&gt; for <a class=\"struct\" href=\"rococo_runtime/struct.RuntimeApiImpl.html\" title=\"struct rococo_runtime::RuntimeApiImpl\">RuntimeApiImpl</a>&lt;__SrApiBlock__, RuntimeApiImplCall&gt;<span class=\"where fmt-newline\">where\n    RuntimeApiImplCall::StateBackend: StateBackend&lt;HashingFor&lt;__SrApiBlock__&gt;&gt;,\n    <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.reference.html\">&amp;'static RuntimeApiImplCall</a>: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/marker/trait.Send.html\" title=\"trait core::marker::Send\">Send</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;&lt;&lt;<a class=\"struct\" href=\"rococo_runtime/struct.Runtime.html\" title=\"struct rococo_runtime::Runtime\">Runtime</a> as Config&gt;::Hashing as Hash&gt;::Output, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;LeafIndex, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.BlockNumber.html\" title=\"type polkadot_core_primitives::BlockNumber\">BlockNumber</a>&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/option/enum.Option.html\" title=\"enum core::option::Option\">Option</a>&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.BlockNumber.html\" title=\"type polkadot_core_primitives::BlockNumber\">BlockNumber</a>&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;(<a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;EncodableOpaqueLeaf&gt;, Proof&lt;&lt;&lt;<a class=\"struct\" href=\"rococo_runtime/struct.Runtime.html\" title=\"struct rococo_runtime::Runtime\">Runtime</a> as Config&gt;::Hashing as Hash&gt;::Output&gt;), Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;EncodableOpaqueLeaf&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    Proof&lt;&lt;&lt;<a class=\"struct\" href=\"rococo_runtime/struct.Runtime.html\" title=\"struct rococo_runtime::Runtime\">Runtime</a> as Config&gt;::Hashing as Hash&gt;::Output&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.unit.html\">()</a>, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    &lt;&lt;<a class=\"struct\" href=\"rococo_runtime/struct.Runtime.html\" title=\"struct rococo_runtime::Runtime\">Runtime</a> as Config&gt;::Hashing as Hash&gt;::Output: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    __SrApiBlock__::Header: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,</span>"]],
"westend_runtime":[["impl&lt;__SrApiBlock__: BlockT + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>, RuntimeApiImplCall: CallApiAt&lt;__SrApiBlock__&gt; + 'static&gt; MmrApi&lt;__SrApiBlock__, H256, <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.u32.html\">u32</a>&gt; for <a class=\"struct\" href=\"westend_runtime/struct.RuntimeApiImpl.html\" title=\"struct westend_runtime::RuntimeApiImpl\">RuntimeApiImpl</a>&lt;__SrApiBlock__, RuntimeApiImplCall&gt;<span class=\"where fmt-newline\">where\n    RuntimeApiImplCall::StateBackend: StateBackend&lt;HashingFor&lt;__SrApiBlock__&gt;&gt;,\n    <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.reference.html\">&amp;'static RuntimeApiImplCall</a>: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/marker/trait.Send.html\" title=\"trait core::marker::Send\">Send</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;&lt;&lt;<a class=\"struct\" href=\"westend_runtime/struct.Runtime.html\" title=\"struct westend_runtime::Runtime\">Runtime</a> as Config&gt;::Hashing as Hash&gt;::Output, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;LeafIndex, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.BlockNumber.html\" title=\"type polkadot_core_primitives::BlockNumber\">BlockNumber</a>&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/option/enum.Option.html\" title=\"enum core::option::Option\">Option</a>&lt;<a class=\"type\" href=\"polkadot_core_primitives/type.BlockNumber.html\" title=\"type polkadot_core_primitives::BlockNumber\">BlockNumber</a>&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;(<a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;EncodableOpaqueLeaf&gt;, Proof&lt;&lt;&lt;<a class=\"struct\" href=\"westend_runtime/struct.Runtime.html\" title=\"struct westend_runtime::Runtime\">Runtime</a> as Config&gt;::Hashing as Hash&gt;::Output&gt;), Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"struct\" href=\"https://doc.rust-lang.org/1.70.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;EncodableOpaqueLeaf&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    Proof&lt;&lt;&lt;<a class=\"struct\" href=\"westend_runtime/struct.Runtime.html\" title=\"struct westend_runtime::Runtime\">Runtime</a> as Config&gt;::Hashing as Hash&gt;::Output&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    <a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.unit.html\">()</a>, Error&gt;: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    &lt;&lt;<a class=\"struct\" href=\"westend_runtime/struct.Runtime.html\" title=\"struct westend_runtime::Runtime\">Runtime</a> as Config&gt;::Hashing as Hash&gt;::Output: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,\n    __SrApiBlock__::Header: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.UnwindSafe.html\" title=\"trait core::panic::unwind_safe::UnwindSafe\">UnwindSafe</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/panic/unwind_safe/trait.RefUnwindSafe.html\" title=\"trait core::panic::unwind_safe::RefUnwindSafe\">RefUnwindSafe</a>,</span>"]]
};if (window.register_implementors) {window.register_implementors(implementors);} else {window.pending_implementors = implementors;}})()