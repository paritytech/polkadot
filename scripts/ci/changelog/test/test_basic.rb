# frozen_string_literal: true

require_relative '../lib/changelog'
require 'test/unit'

class TestChangelog < Test::Unit::TestCase
  def test_get_dep_ref_polkadot
    c = SubRef.new('paritytech/polkadot')
    ref = '13c2695'
    package = 'sc-cli'
    result = c.get_dependency_reference(ref, package)
    assert_equal('7db0768a85dc36a3f2a44d042b32f3715c00a90d', result)
  end

  def test_get_dep_ref_invalid_ref
    c = SubRef.new('paritytech/polkadot')
    ref = '9999999'
    package = 'sc-cli'
    assert_raise do
      c.get_dependency_reference(ref, package)
    end
  end
end
