# frozen_string_literal: true

require 'base64'
require 'changelogerator'
require 'erb'
require 'git'
require 'json'
require 'octokit'
require 'toml'
require 'pry'
require_relative './lib.rb'

current_ref = ENV['GITHUB_REF']
token = ENV['GITHUB_TOKEN']
github_client = Octokit::Client.new(
  access_token: token
)

polkadot_path = ENV['GITHUB_WORKSPACE'] + '/polkadot/'

# Generate an ERB renderer based on the template .erb file
renderer = ERB.new(
  File.read(ENV['GITHUB_WORKSPACE'] + '/polkadot/scripts/github/polkadot_release.erb'),
  trim_mode: '<>'
)

# get ref of last polkadot release
last_ref = "refs/tags/" + github_client.latest_release(ENV['GITHUB_REPOSITORY']).tag_name

polkadot_cl = Changelog.new(
  'paritytech/polkadot', last_ref, current_ref, token: token
)

# Gets the substrate commit hash used for a given polkadot ref
def get_substrate_commit(client, ref)
  cargo = TOML::Parser.new(
    Base64.decode64(
      client.contents(
        ENV['GITHUB_REPOSITORY'],
        path: 'Cargo.lock',
        query: { ref: "#{ref}"}
      ).content
    )
  ).parsed
  cargo['package'].find { |p| p['name'] == 'sc-cli' }['source'].split('#').last
end

substrate_prev_sha = get_substrate_commit(github_client, last_ref)
substrate_cur_sha = get_substrate_commit(github_client, current_ref)

substrate_cl = Changelog.new(
  'paritytech/substrate', substrate_prev_sha, substrate_cur_sha,
  token: token,
  prefix: true
)

all_changes = polkadot_cl.changes + substrate_cl.changes

# Set all the variables needed for a release

misc_changes = Changelog.changes_with_label(all_changes, 'B1-releasenotes')
client_changes = Changelog.changes_with_label(all_changes, 'B5-clientnoteworthy')
runtime_changes = Changelog.changes_with_label(all_changes, 'B7-runtimenoteworthy')

release_priority = Changelog.highest_priority_for_changes(all_changes)

# Pulled from the previous Github step
rustc_stable = ENV['RUSTC_STABLE']
rustc_nightly = ENV['RUSTC_NIGHTLY']
binding.pry
polkadot_runtime = get_runtime('polkadot', polkadot_path)
kusama_runtime = get_runtime('kusama', polkadot_path)
westend_runtime = get_runtime('westend', polkadot_path)

# These json files should have been downloaded as part of the build-runtimes
# github action

polkadot_json = JSON.parse(
  File.read(
    ENV['GITHUB_WORKSPACE'] + '/polkadot-srtool-json/srtool_output.json'
  )
)

kusama_json = JSON.parse(
  File.read(
    ENV['GITHUB_WORKSPACE'] + '/kusama-srtool-json/srtool_output.json'
  )
)

puts renderer.result
