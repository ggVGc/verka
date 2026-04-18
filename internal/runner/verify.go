package runner

// DefaultVerifyCmd returns the command used when a verification node does not
// specify one.
func DefaultVerifyCmd() []string {
	return []string{"go", "test", "./..."}
}
