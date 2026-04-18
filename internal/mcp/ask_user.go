package mcp

import (
	"bufio"
	"context"
	"fmt"
	"os"
	"strings"
	"sync"

	mcpsdk "github.com/modelcontextprotocol/go-sdk/mcp"
)

// askUserArgs is the tool input.
type askUserArgs struct {
	Question string   `json:"question" jsonschema:"the question to ask the user"`
	Options  []string `json:"options,omitempty" jsonschema:"optional list of numbered choices; the user may still type a free-form answer"`
}

type askUserResult struct {
	Answer string `json:"answer"`
}

// askUserMu serializes terminal I/O across concurrent handler invocations.
var askUserMu sync.Mutex

// promptUser is the actual "ask a question, read a line" implementation.
// Tests override this variable to avoid touching /dev/tty.
var promptUser = ttyPromptUser

func (h *handlers) askUser(ctx context.Context, req *mcpsdk.CallToolRequest, in askUserArgs) (*mcpsdk.CallToolResult, askUserResult, error) {
	if strings.TrimSpace(in.Question) == "" {
		return errf("question must not be empty"), askUserResult{}, nil
	}
	askUserMu.Lock()
	defer askUserMu.Unlock()

	// Run the blocking prompt in a goroutine so we can honor context
	// cancellation (e.g., user Ctrl-Cs the orchestrator).
	type result struct {
		answer string
		err    error
	}
	ch := make(chan result, 1)
	go func() {
		a, err := promptUser(in.Question, in.Options)
		ch <- result{a, err}
	}()
	select {
	case <-ctx.Done():
		return errf("ask_user canceled: %v", ctx.Err()), askUserResult{}, nil
	case r := <-ch:
		if r.err != nil {
			return errf("ask_user: %v", r.err), askUserResult{}, nil
		}
		out := askUserResult{Answer: r.answer}
		return ok(out), out, nil
	}
}

// ttyPromptUser is the real implementation: opens /dev/tty so the prompt
// works even when the process's stdin/stdout are attached to an MCP pipe.
func ttyPromptUser(question string, options []string) (string, error) {
	tty, err := os.OpenFile("/dev/tty", os.O_RDWR, 0)
	if err != nil {
		return "", fmt.Errorf("open /dev/tty: %w (ask_user requires a terminal)", err)
	}
	defer tty.Close()

	fmt.Fprintln(tty)
	fmt.Fprintln(tty, "── agent asks ───────────────────────────────────────────────────────")
	fmt.Fprintln(tty, question)
	for i, opt := range options {
		fmt.Fprintf(tty, "  %d. %s\n", i+1, opt)
	}
	fmt.Fprint(tty, "> ")

	reader := bufio.NewReader(tty)
	line, err := reader.ReadString('\n')
	if err != nil {
		return "", fmt.Errorf("read from tty: %w", err)
	}
	answer := strings.TrimRight(line, "\r\n")
	fmt.Fprintln(tty, "──────────────────────────────────────────────────────────────────────")
	return answer, nil
}
