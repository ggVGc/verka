package orchestrator

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"strings"
)

// filterStream reads Claude CLI's stream-json output (one JSON object per
// line), and emits human-readable progress lines to `out`. It surfaces:
//   - assistant text turns (the model's reasoning and final summary)
//   - MCP tool calls with a single-line "tool(args summary)" and
//   - the short result status (ok/failure).
//
// Unknown event shapes are tolerated — the raw stream is already being tee'd
// to the orchestrator log for post-mortem.
func filterStream(r io.Reader, out io.Writer) error {
	scanner := bufio.NewScanner(r)
	scanner.Buffer(make([]byte, 0, 64*1024), 8*1024*1024)
	for scanner.Scan() {
		line := scanner.Bytes()
		if len(line) == 0 {
			continue
		}
		var evt map[string]any
		if err := json.Unmarshal(line, &evt); err != nil {
			// Non-JSON line — forward as-is (rare but possible on stderr/stdout
			// crossover; claude should keep these separated).
			fmt.Fprintln(out, string(line))
			continue
		}
		renderEvent(out, evt)
	}
	return scanner.Err()
}

func renderEvent(out io.Writer, evt map[string]any) {
	switch evt["type"] {
	case "system":
		// Startup metadata; keep quiet unless there's a useful subtype.
		if sub, _ := evt["subtype"].(string); sub == "init" {
			fmt.Fprintln(out, "agent: initialized")
		}
	case "assistant":
		// The model's turn. May contain text and/or tool_use blocks.
		renderAssistant(out, evt)
	case "user":
		// Tool results coming back from MCP to the model.
		renderUser(out, evt)
	case "result":
		if s, ok := evt["result"].(string); ok {
			trimmed := strings.TrimSpace(s)
			if trimmed != "" {
				fmt.Fprintln(out, "---")
				fmt.Fprintln(out, trimmed)
			}
		}
	}
}

func renderAssistant(out io.Writer, evt map[string]any) {
	msg, _ := evt["message"].(map[string]any)
	if msg == nil {
		return
	}
	content, _ := msg["content"].([]any)
	for _, block := range content {
		b, _ := block.(map[string]any)
		switch b["type"] {
		case "text":
			text, _ := b["text"].(string)
			text = strings.TrimSpace(text)
			if text != "" {
				fmt.Fprintln(out, "agent:", oneLine(text, 400))
			}
		case "tool_use":
			name, _ := b["name"].(string)
			input, _ := b["input"].(map[string]any)
			fmt.Fprintf(out, "→ %s(%s)\n", shortToolName(name), summarizeArgs(input))
		}
	}
}

func renderUser(out io.Writer, evt map[string]any) {
	msg, _ := evt["message"].(map[string]any)
	if msg == nil {
		return
	}
	content, _ := msg["content"].([]any)
	for _, block := range content {
		b, _ := block.(map[string]any)
		if b["type"] != "tool_result" {
			continue
		}
		isError, _ := b["is_error"].(bool)
		text := toolResultText(b["content"])
		if isError {
			fmt.Fprintf(out, "  ✗ %s\n", oneLine(text, 300))
		} else {
			fmt.Fprintf(out, "  ✓ %s\n", oneLine(text, 160))
		}
	}
}

// shortToolName turns `mcp__llaundry__create_node` into `create_node`.
func shortToolName(full string) string {
	if i := strings.LastIndex(full, "__"); i >= 0 {
		return full[i+2:]
	}
	return full
}

// summarizeArgs collapses a JSON-y arg map into a compact "k=v,k=v" string.
// For long strings (content blobs), only the length is shown.
func summarizeArgs(args map[string]any) string {
	if len(args) == 0 {
		return ""
	}
	var parts []string
	for k, v := range args {
		parts = append(parts, fmt.Sprintf("%s=%s", k, summarizeValue(v)))
	}
	return strings.Join(parts, ", ")
}

func summarizeValue(v any) string {
	switch t := v.(type) {
	case string:
		if len(t) > 40 {
			return fmt.Sprintf("<%d bytes>", len(t))
		}
		return fmt.Sprintf("%q", t)
	case map[string]any:
		return fmt.Sprintf("{%d keys}", len(t))
	case []any:
		return fmt.Sprintf("[%d items]", len(t))
	default:
		return fmt.Sprint(v)
	}
}

// toolResultText normalizes the varied shapes `content` may take (string, or a
// list of content blocks) into a single rendered string.
func toolResultText(content any) string {
	switch c := content.(type) {
	case string:
		return c
	case []any:
		var parts []string
		for _, block := range c {
			b, ok := block.(map[string]any)
			if !ok {
				continue
			}
			if b["type"] == "text" {
				if s, ok := b["text"].(string); ok {
					parts = append(parts, s)
				}
			}
		}
		return strings.Join(parts, " ")
	default:
		b, _ := json.Marshal(content)
		return string(b)
	}
}

func oneLine(s string, max int) string {
	s = strings.ReplaceAll(s, "\n", " ")
	s = strings.TrimSpace(s)
	if len(s) > max {
		return s[:max-1] + "…"
	}
	return s
}
