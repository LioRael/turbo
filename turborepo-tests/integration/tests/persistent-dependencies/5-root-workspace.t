# Setup
  $ . ${TESTDIR}/../../../helpers/setup_integration_test.sh $(pwd) persistent_dependencies/5-root-workspace

# WorkspaceGraph: no package dependencies
#
# Task Graph:
# build
# └── //#dev
#
# With this workspace graph, that means:
#
# app-a#build
# └── //#dev
#
  $ ${TURBO} run build
   ERROR  run failed: error preparing engine: Invalid persistent task configuration:
  "//#dev" is a persistent task, "app-a#build" cannot depend on it
  [1]