# frozen_string_literal: true

# A collection of helper functions that might be useful for various scripts

# Gets the runtime version for a given runtime.
# Optionally accepts a path that is the root of the project which defaults to
# the current working directory
def get_runtime(runtime, path = '.')
  File.open(path + "/runtime/#{runtime}/src/lib.rs") do |f|
    f.find { |l| l =~ /spec_version/ }.match(/[0-9]+/)[0]
  end
end
