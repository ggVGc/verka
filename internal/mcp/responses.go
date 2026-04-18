package mcp

import (
	"encoding/json"
	"fmt"

	mcpsdk "github.com/modelcontextprotocol/go-sdk/mcp"
)

// ok wraps the output payload into a CallToolResult carrying a JSON text
// content block so clients without structured-output support still see
// something useful.
func ok(v any) *mcpsdk.CallToolResult {
	b, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return textErr(err.Error())
	}
	return &mcpsdk.CallToolResult{
		Content: []mcpsdk.Content{&mcpsdk.TextContent{Text: string(b)}},
	}
}

// textErr returns a CallToolResult marked as an error with the given message.
func textErr(msg string) *mcpsdk.CallToolResult {
	return &mcpsdk.CallToolResult{
		IsError: true,
		Content: []mcpsdk.Content{&mcpsdk.TextContent{Text: msg}},
	}
}

// errf formats an error into a tool-call error result.
func errf(format string, args ...any) *mcpsdk.CallToolResult {
	return textErr(fmt.Sprintf(format, args...))
}

// truncate clips s to at most max bytes, marking the clip if it happened.
func truncate(s string, max int) string {
	if len(s) <= max {
		return s
	}
	return s[:max] + "\n…[truncated]"
}
