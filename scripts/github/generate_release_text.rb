# frozen_string_literal: true

require 'changelogerator'
require 'git'
require 'erb'

polkadot_path = ENV['GITHUB_WORKSPACE']+"/polkadot"
pg = Git.open(polkadot_path)

version = ENV['GITHUB_REF']
token = ENV['GITHUB_TOKEN']

# get last polkadot version. Use handy Gem::Version for sorting by version
last_version = pg
              .tags
              .map(&:name)
              .grep(/^v\d+\.\d+\.\d+.*$/)
              .sort_by { |v| Gem::Version.new(v.slice(1...)) }[-2]

# TODO: fill in these variables
polkadot_runtime = '123'
kusama_runtime = '123'
westend_runtime = '123'
rustc_stable = 'v1.2.3'
rustc_nightly = 'v1.2.3'

polkadot_cl = Changelog.new(
  's3krit/polkadot', version, last_version, token: token
)

# Get prev and cur substrate shas HACKY!
substrate_prev_sha = ''
substrate_cur_sha = ''
patch = pg.diff(last_version, version).path('Cargo.lock').patch.split("\n")
patch.each_with_index do |x, i|
  if x =~ / name = "sc-cli"/
    substrate_prev_sha = patch[i + 2].split('#').last.chomp('"')
    substrate_cur_sha = patch[i + 4].split('#').last.chomp('"')
  end
end

substrate_cl = Changelog.new(
  'paritytech/substrate', substrate_prev_sha, substrate_cur_sha,
  token: token,
  prefix: true
)

all_changes = polkadot_cl.changes + substrate_cl.changes

misc_changes = Changelog.changes_with_label(all_changes, 'B1-releasenotes')
client_changes = Changelog.changes_with_label(all_changes, 'B5-clientnoteworthy')
runtime_changes = Changelog.changes_with_label(all_changes, 'B7-runtimenoteworthy')

release_priority = Changelog.highest_priority_for_changes(all_changes)

renderer = ERB.new(File.read('polkadot_release.erb'), trim_mode: '<>')
renderer.result
