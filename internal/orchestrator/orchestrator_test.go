package orchestrator

import (
	"bytes"
	"encoding/json"
	"strings"
	"testing"
)

func TestBuildPlanWithDescriptionID(t *testing.T) {
	cfg := DefaultConfig()
	cfg.Cwd = "/tmp/demo"
	cfg.LlaundryBin = "/usr/local/bin/llaundry"
	cfg.DescriptionID = "D1"

	p, err := BuildPlan(cfg)
	if err != nil {
		t.Fatalf("BuildPlan: %v", err)
	}

	joined := strings.Join(p.Argv, " ")
	for _, want := range []string{
		"--bare",
		"--allowedTools mcp__llaundry__*",
		"--permission-mode dontAsk",
		"--output-format stream-json",
		"--max-turns 50",
	} {
		if !strings.Contains(joined, want) {
			t.Errorf("argv missing %q\n  got: %s", want, joined)
		}
	}

	if !strings.Contains(p.UserPrompt, "D1") {
		t.Errorf("user prompt missing description ID: %q", p.UserPrompt)
	}

	var mcp map[string]any
	if err := json.Unmarshal(p.MCPConfig, &mcp); err != nil {
		t.Fatalf("mcp config not valid JSON: %v", err)
	}
	servers, _ := mcp["mcpServers"].(map[string]any)
	llaundry, ok := servers["llaundry"].(map[string]any)
	if !ok {
		t.Fatalf("mcp-config missing llaundry server: %s", p.MCPConfig)
	}
	if llaundry["command"] != "/usr/local/bin/llaundry" {
		t.Errorf("mcp-config wrong command: %v", llaundry["command"])
	}
	args, _ := llaundry["args"].([]any)
	if len(args) != 3 || args[0] != "mcp" || args[1] != "-root" || args[2] != "/tmp/demo" {
		t.Errorf("mcp-config wrong args: %v", args)
	}

	if !strings.Contains(p.SystemPrompt, "llaundry orchestrator") {
		t.Error("system prompt missing role framing")
	}
	for _, want := range []string{
		"run_verification",
		"ask_user",
		"Phase 1 — Clarify",
		"Phase 2 — Propose and confirm tasks",
		"Only after explicit approval",
	} {
		if !strings.Contains(p.SystemPrompt, want) {
			t.Errorf("system prompt missing %q", want)
		}
	}
}

func TestBuildPlanWithUserPromptOnly(t *testing.T) {
	cfg := DefaultConfig()
	cfg.Cwd = "/tmp/demo"
	cfg.LlaundryBin = "/usr/local/bin/llaundry"
	cfg.UserPrompt = "reverse a string CLI"

	p, err := BuildPlan(cfg)
	if err != nil {
		t.Fatalf("BuildPlan: %v", err)
	}
	if !strings.Contains(p.UserPrompt, "reverse a string CLI") {
		t.Errorf("user prompt missing brief: %q", p.UserPrompt)
	}
	if !strings.Contains(p.UserPrompt, "Create a new llaundry description") {
		t.Errorf("user prompt missing bootstrap framing: %q", p.UserPrompt)
	}
}

func TestValidateRejectsEmpty(t *testing.T) {
	cfg := DefaultConfig()
	if err := cfg.Validate(); err == nil {
		t.Fatal("expected validation error for empty Cwd")
	}
	cfg.Cwd = "/tmp/demo"
	if err := cfg.Validate(); err == nil {
		t.Fatal("expected validation error when both DescriptionID and UserPrompt are empty")
	}
}

func TestSystemPromptOverride(t *testing.T) {
	cfg := DefaultConfig()
	cfg.Cwd = "/tmp/demo"
	cfg.LlaundryBin = "/bin/llaundry"
	cfg.DescriptionID = "D1"
	cfg.SystemPromptOverride = "custom prompt"
	p, err := BuildPlan(cfg)
	if err != nil {
		t.Fatal(err)
	}
	if p.SystemPrompt != "custom prompt" {
		t.Errorf("override ignored: %q", p.SystemPrompt)
	}
}

func TestFilterStreamSmoke(t *testing.T) {
	events := []map[string]any{
		{"type": "system", "subtype": "init"},
		{
			"type": "assistant",
			"message": map[string]any{
				"content": []any{
					map[string]any{"type": "text", "text": "surveying state"},
					map[string]any{
						"type":  "tool_use",
						"name":  "mcp__llaundry__list_nodes",
						"input": map[string]any{"stale": true},
					},
				},
			},
		},
		{
			"type": "user",
			"message": map[string]any{
				"content": []any{
					map[string]any{
						"type":     "tool_result",
						"is_error": false,
						"content": []any{
							map[string]any{"type": "text", "text": `{"nodes":[]}`},
						},
					},
				},
			},
		},
		{"type": "result", "result": "done: 1 task, 1 build"},
	}
	var in bytes.Buffer
	for _, e := range events {
		b, _ := json.Marshal(e)
		in.Write(b)
		in.WriteByte('\n')
	}
	var out bytes.Buffer
	if err := filterStream(&in, &out); err != nil {
		t.Fatal(err)
	}
	got := out.String()
	for _, want := range []string{
		"agent: initialized",
		"agent: surveying state",
		"→ list_nodes(stale=true)",
		"✓",
		"done: 1 task, 1 build",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("stream output missing %q\nfull output:\n%s", want, got)
		}
	}
}
