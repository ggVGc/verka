package runner

// Default command selection happens in internal/mcp/run_tools.go because it
// depends on the number of implementation dependencies; this file exists so
// future refinements (multi-impl builds, alternative toolchains) have an
// obvious home.
