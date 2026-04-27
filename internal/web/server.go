// Package web serves an interactive visualization of the llaundry graph.
package web

import (
	"context"
	"embed"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"time"

	"github.com/ggvgc/llaundry/internal/model"
	"github.com/ggvgc/llaundry/internal/store"
	"github.com/ggvgc/llaundry/internal/workspace"
)

//go:embed static
var staticFS embed.FS

// Server wraps an http.Handler that exposes llaundry graph data as JSON and
// serves the embedded single-page viewer.
type Server struct {
	ws  *workspace.Workspace
	mux *http.ServeMux
}

func New(ws *workspace.Workspace) *Server {
	s := &Server{ws: ws, mux: http.NewServeMux()}
	s.routes()
	return s
}

func (s *Server) Handler() http.Handler { return s.mux }

func (s *Server) routes() {
	sub, err := fs.Sub(staticFS, "static")
	if err != nil {
		panic(err)
	}
	s.mux.Handle("/", http.FileServer(http.FS(sub)))
	s.mux.HandleFunc("/api/graph", s.handleGraph)
	s.mux.HandleFunc("/api/node/", s.handleNode)
}

type apiNode struct {
	ID          string          `json:"id"`
	Type        model.NodeType  `json:"type"`
	Status      model.Status    `json:"status"`
	ContentHash string          `json:"content_hash"`
	Content     json.RawMessage `json:"content,omitempty"`
	CreatedAt   time.Time       `json:"created_at"`
	UpdatedAt   time.Time       `json:"updated_at"`
	Stale       bool            `json:"stale,omitempty"`
}

type apiEdge struct {
	Src  string         `json:"src"`
	Dst  string         `json:"dst"`
	Kind model.EdgeKind `json:"kind"`
}

type apiGraphResp struct {
	Nodes []apiNode `json:"nodes"`
	Edges []apiEdge `json:"edges"`
}

func (s *Server) handleGraph(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	st := s.ws.Store()

	nodes, err := st.ListNodes(ctx, store.NodeFilter{})
	if err != nil {
		httpError(w, err)
		return
	}
	staleIDs, err := st.StaleNodes(ctx)
	if err != nil {
		httpError(w, err)
		return
	}
	staleSet := make(map[string]bool, len(staleIDs))
	for _, id := range staleIDs {
		staleSet[id] = true
	}
	staleImplIDs, _ := st.StaleImplementations(ctx)
	for _, id := range staleImplIDs {
		staleSet[id] = true
	}

	resp := apiGraphResp{Nodes: make([]apiNode, 0, len(nodes))}
	for _, n := range nodes {
		resp.Nodes = append(resp.Nodes, apiNode{
			ID:          n.ID,
			Type:        n.Type,
			Status:      n.Status,
			ContentHash: n.ContentHash,
			CreatedAt:   n.CreatedAt,
			UpdatedAt:   n.UpdatedAt,
			Stale:       staleSet[n.ID],
		})
	}

	seen := make(map[string]bool)
	for _, n := range nodes {
		edges, err := st.EdgesFor(ctx, n.ID)
		if err != nil {
			httpError(w, err)
			return
		}
		for _, e := range edges {
			key := e.Src + "|" + e.Dst + "|" + string(e.Kind)
			if seen[key] {
				continue
			}
			seen[key] = true
			resp.Edges = append(resp.Edges, apiEdge{Src: e.Src, Dst: e.Dst, Kind: e.Kind})
		}
	}
	writeJSON(w, resp)
}

type apiNodeDetail struct {
	apiNode
	Files []model.FileRecord `json:"files"`
	Edges []apiEdge          `json:"edges"`
	Run   *model.Run         `json:"latest_run,omitempty"`
}

func (s *Server) handleNode(w http.ResponseWriter, r *http.Request) {
	id := r.URL.Path[len("/api/node/"):]
	if id == "" {
		http.Error(w, "missing node id", http.StatusBadRequest)
		return
	}
	ctx := r.Context()
	st := s.ws.Store()

	n, err := st.GetNode(ctx, id)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			http.Error(w, "node not found", http.StatusNotFound)
			return
		}
		httpError(w, err)
		return
	}
	files, err := st.ListFiles(ctx, id)
	if err != nil {
		httpError(w, err)
		return
	}
	edges, err := st.EdgesFor(ctx, id)
	if err != nil {
		httpError(w, err)
		return
	}
	apiEdges := make([]apiEdge, 0, len(edges))
	for _, e := range edges {
		apiEdges = append(apiEdges, apiEdge{Src: e.Src, Dst: e.Dst, Kind: e.Kind})
	}
	run, err := st.GetLatestRun(ctx, id)
	if err != nil && !errors.Is(err, store.ErrNotFound) {
		httpError(w, err)
		return
	}
	stale := false
	if staleIDs, err := st.StaleNodes(ctx); err == nil {
		for _, sid := range staleIDs {
			if sid == id {
				stale = true
				break
			}
		}
	}
	if !stale {
		if staleImplIDs, err := st.StaleImplementations(ctx); err == nil {
			for _, sid := range staleImplIDs {
				if sid == id {
					stale = true
					break
				}
			}
		}
	}
	writeJSON(w, apiNodeDetail{
		apiNode: apiNode{
			ID: n.ID, Type: n.Type, Status: n.Status,
			ContentHash: n.ContentHash, Content: n.Content,
			CreatedAt: n.CreatedAt, UpdatedAt: n.UpdatedAt, Stale: stale,
		},
		Files: files,
		Edges: apiEdges,
		Run:   run,
	})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	enc := json.NewEncoder(w)
	enc.SetIndent("", "  ")
	if err := enc.Encode(v); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
	}
}

func httpError(w http.ResponseWriter, err error) {
	http.Error(w, err.Error(), http.StatusInternalServerError)
}

// Serve runs an HTTP server on addr backed by ws. It blocks until ctx is
// canceled or the server errors.
func Serve(ctx context.Context, ws *workspace.Workspace, addr string) error {
	s := New(ws)
	srv := &http.Server{
		Addr:              addr,
		Handler:           s.Handler(),
		ReadHeaderTimeout: 10 * time.Second,
	}
	errCh := make(chan error, 1)
	go func() { errCh <- srv.ListenAndServe() }()
	fmt.Printf("llaundry web: listening on http://%s\n", addr)

	select {
	case <-ctx.Done():
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = srv.Shutdown(shutdownCtx)
		return nil
	case err := <-errCh:
		if errors.Is(err, http.ErrServerClosed) {
			return nil
		}
		return err
	}
}
