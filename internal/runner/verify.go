package runner

// DefaultVerifyCmd is the command run_verification executes against an
// implementation's source directory.
func DefaultVerifyCmd() []string {
	return []string{"go", "test", "./..."}
}

// DefaultBuildCmd is the command run_build executes against an
// implementation's source directory, writing artifacts into artifactDir.
func DefaultBuildCmd(artifactDir string) []string {
	return []string{"go", "build", "-o", artifactDir + "/", "./..."}
}
