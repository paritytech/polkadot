# frozen_string_literal: true

require 'base64'
require 'changelogerator'
require 'erb'
require 'git'
require 'json'
require 'octokit'
require 'toml'
require_relative './lib.rb'

# A logger only active when NOT running in CI
def logger(s)
  puts "▶ DEBUG: %s" % [s] if ENV['CI'] != 'true'
end

# Check if all the required ENV are set
# This is especially convenient when testing locally
def check_env()
  if ENV['CI'] != 'true' then
    logger("Running locally")
    vars = ['GITHUB_REF', 'GITHUB_TOKEN', 'GITHUB_WORKSPACE', 'GITHUB_REPOSITORY', 'RUSTC_STABLE', 'RUSTC_NIGHTLY']
    vars.each { |x|
      env = (ENV[x] || "")
      if env.length > 0 then
        logger("- %s:\tset: %s, len: %d" % [x, env.length > 0 || false, env.length])
      else
        logger("- %s:\tset: %s, len: %d" % [x, env.length > 0 || false, env.length])
      end
    }
  end
end

check_env()

current_ref = ENV['GITHUB_REF']
token = ENV['GITHUB_TOKEN']

logger("Connecting to Github")
github_client = Octokit::Client.new(
  access_token: token
)

polkadot_path = ENV['GITHUB_WORKSPACE'] + '/polkadot/'

# Generate an ERB renderer based on the template .erb file
renderer = ERB.new(
  File.read(File.join(polkadot_path, 'scripts/ci/github/polkadot_release.erb')),
  trim_mode: '<>'
)

# get ref of last polkadot release
last_ref = 'refs/tags/' + github_client.latest_release(ENV['GITHUB_REPOSITORY']).tag_name
logger("Last ref: " + last_ref)

logger("Generate changelog for Polkadot")
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
        query: { ref: ref.to_s }
      ).content
    )
  ).parsed
  cargo['package'].find { |p| p['name'] == 'sc-cli' }['source'].split('#').last
end

substrate_prev_sha = get_substrate_commit(github_client, last_ref)
substrate_cur_sha = get_substrate_commit(github_client, current_ref)

logger("Generate changelog for Substrate")
substrate_cl = Changelog.new(
  'paritytech/substrate', substrate_prev_sha, substrate_cur_sha,
  token: token,
  prefix: true
)

# Combine all changes into a single array and filter out companions
all_changes = (polkadot_cl.changes + substrate_cl.changes).reject do |c|
  c[:title] =~ /[Cc]ompanion/
end

# Set all the variables needed for a release

misc_changes = Changelog.changes_with_label(all_changes, 'B1-releasenotes')
client_changes = Changelog.changes_with_label(all_changes, 'B5-clientnoteworthy')
runtime_changes = Changelog.changes_with_label(all_changes, 'B7-runtimenoteworthy')

# Add the audit status for runtime changes
runtime_changes.each do |c|
  if c[:labels].any? { |l| l[:name] == 'D1-audited 👍' }
    c[:pretty_title] = "✅ `audited` #{c[:pretty_title]}"
    next
  end
  if c[:labels].any? { |l| l[:name] == 'D2-notlive 💤' }
    c[:pretty_title] = "✅ `not live` #{c[:pretty_title]}"
    next
  end
  if c[:labels].any? { |l| l[:name] == 'D3-trivial 🧸' }
    c[:pretty_title] = "✅ `trivial` #{c[:pretty_title]}"
    next
  end
  if c[:labels].any? { |l| l[:name] == 'D5-nicetohaveaudit ⚠️' }
    c[:pretty_title] = "⏳ `pending non-critical audit` #{c[:pretty_title]}"
    next
  end
  if c[:labels].any? { |l| l[:name] == 'D9-needsaudit 👮' }
    c[:pretty_title] = "❌ `AWAITING AUDIT` #{c[:pretty_title]}"
    next
  end
  c[:pretty_title] = "⭕️ `unknown audit requirements` #{c[:pretty_title]}"
end

# The priority of users upgraded is determined by the highest-priority
# *Client* change
release_priority = Changelog.highest_priority_for_changes(client_changes)

# Pulled from the previous Github step
rustc_stable = ENV['RUSTC_STABLE']
rustc_nightly = ENV['RUSTC_NIGHTLY']
polkadot_runtime = get_runtime('polkadot', polkadot_path)
kusama_runtime = get_runtime('kusama', polkadot_path)
westend_runtime = get_runtime('westend', polkadot_path)
rococo_runtime = get_runtime('rococo', polkadot_path)

# These json files should have been downloaded as part of the build-runtimes
# github action

polkadot_json = JSON.parse(
  File.read(
    "#{ENV['GITHUB_WORKSPACE']}/polkadot-srtool-json/polkadot_srtool_output.json"
  )
)

kusama_json = JSON.parse(
  File.read(
    "#{ENV['GITHUB_WORKSPACE']}/kusama-srtool-json/kusama_srtool_output.json"
  )
)

puts renderer.result
