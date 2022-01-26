# frozen_string_literal: true

# Gets the runtime version for a given runtime from the filesystem.
# Optionally accepts a path that is the root of the project which defaults to
# the current working directory
def get_runtime(runtime: nil, path: '.', runtime_dir: 'runtime')
  File.open(path + "/#{runtime_dir}/#{runtime}/src/lib.rs") do |f|
    f.find { |l| l =~ /spec_version/ }.match(/[0-9]+/)[0]
  end
end
