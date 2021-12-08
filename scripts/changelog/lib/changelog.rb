# frozen_string_literal: true

# A Class to find Substrate references
class SubRef
  require 'octokit'
  require 'toml'

  attr_reader :client, :repository

  def initialize(github_repo)
    @client = Octokit::Client.new(
      access_token: ENV['GITHUB_TOKEN']
    )
    @repository = @client.repository(github_repo)
  end

  # This function checks the Cargo.lock of a given
  # Rust project, for a given package, and fetches
  # the dependency git ref.
  def get_dependency_reference(ref, package)
    cargo = TOML::Parser.new(
      Base64.decode64(
        @client.contents(
          @repository.full_name,
          path: 'Cargo.lock',
          query: { ref: ref.to_s }
        ).content
      )
    ).parsed
    cargo['package'].find { |p| p['name'] == package }['source'].split('#').last
  end
end
