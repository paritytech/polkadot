(function() {var implementors = {
"polkadot_service":[],
"xcm_builder":[["impl&lt;RuntimeOrigin: OriginTrait + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/clone/trait.Clone.html\" title=\"trait core::clone::Clone\">Clone</a>, AccountId: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/convert/trait.Into.html\" title=\"trait core::convert::Into\">Into</a>&lt;[<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.u8.html\">u8</a>; <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.70.0/std/primitive.array.html\">32</a>]&gt;, Network: Get&lt;<a class=\"enum\" href=\"https://doc.rust-lang.org/1.70.0/core/option/enum.Option.html\" title=\"enum core::option::Option\">Option</a>&lt;<a class=\"enum\" href=\"xcm_builder/test_utils/enum.NetworkId.html\" title=\"enum xcm_builder::test_utils::NetworkId\">NetworkId</a>&gt;&gt;&gt; TryConvert&lt;RuntimeOrigin, <a class=\"struct\" href=\"xcm_builder/test_utils/struct.MultiLocation.html\" title=\"struct xcm_builder::test_utils::MultiLocation\">MultiLocation</a>&gt; for <a class=\"struct\" href=\"xcm_builder/struct.SignedToAccountId32.html\" title=\"struct xcm_builder::SignedToAccountId32\">SignedToAccountId32</a>&lt;RuntimeOrigin, AccountId, Network&gt;<span class=\"where fmt-newline\">where\n    RuntimeOrigin::PalletsOrigin: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;SystemRawOrigin&lt;AccountId&gt;&gt; + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/convert/trait.TryInto.html\" title=\"trait core::convert::TryInto\">TryInto</a>&lt;SystemRawOrigin&lt;AccountId&gt;, Error = RuntimeOrigin::PalletsOrigin&gt;,</span>"],["impl&lt;RuntimeOrigin: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/clone/trait.Clone.html\" title=\"trait core::clone::Clone\">Clone</a>, EnsureBodyOrigin: EnsureOrigin&lt;RuntimeOrigin&gt;, Body: Get&lt;<a class=\"enum\" href=\"xcm_builder/test_utils/enum.BodyId.html\" title=\"enum xcm_builder::test_utils::BodyId\">BodyId</a>&gt;&gt; TryConvert&lt;RuntimeOrigin, <a class=\"struct\" href=\"xcm_builder/test_utils/struct.MultiLocation.html\" title=\"struct xcm_builder::test_utils::MultiLocation\">MultiLocation</a>&gt; for <a class=\"struct\" href=\"xcm_builder/struct.OriginToPluralityVoice.html\" title=\"struct xcm_builder::OriginToPluralityVoice\">OriginToPluralityVoice</a>&lt;RuntimeOrigin, EnsureBodyOrigin, Body&gt;"],["impl&lt;RuntimeOrigin: OriginTrait + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/clone/trait.Clone.html\" title=\"trait core::clone::Clone\">Clone</a>, COrigin: GetBacking, Body: Get&lt;<a class=\"enum\" href=\"xcm_builder/test_utils/enum.BodyId.html\" title=\"enum xcm_builder::test_utils::BodyId\">BodyId</a>&gt;&gt; TryConvert&lt;RuntimeOrigin, <a class=\"struct\" href=\"xcm_builder/test_utils/struct.MultiLocation.html\" title=\"struct xcm_builder::test_utils::MultiLocation\">MultiLocation</a>&gt; for <a class=\"struct\" href=\"xcm_builder/struct.BackingToPlurality.html\" title=\"struct xcm_builder::BackingToPlurality\">BackingToPlurality</a>&lt;RuntimeOrigin, COrigin, Body&gt;<span class=\"where fmt-newline\">where\n    RuntimeOrigin::PalletsOrigin: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;COrigin&gt; + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.70.0/core/convert/trait.TryInto.html\" title=\"trait core::convert::TryInto\">TryInto</a>&lt;COrigin, Error = RuntimeOrigin::PalletsOrigin&gt;,</span>"]]
};if (window.register_implementors) {window.register_implementors(implementors);} else {window.pending_implementors = implementors;}})()