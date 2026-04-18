package store

import (
	"context"
	"encoding/json"

	"github.com/ggvgc/llaundry/internal/model"
)

// NodeFilter narrows list_nodes queries. Empty fields are wildcards.
type NodeFilter struct {
	Type   model.NodeType
	Status model.Status
	Parent string // return nodes that are children (EdgeChild) of Parent
	Root   string // return nodes transitively reachable from Root via EdgeChild / EdgeDependsOn
	Stale  bool   // if true, return only stale verification/build nodes
	Limit  int
	Offset int
}

type Store interface {
	CreateNode(ctx context.Context, n *model.Node) error
	GetNode(ctx context.Context, id string) (*model.Node, error)
	ListNodes(ctx context.Context, f NodeFilter) ([]*model.Node, error)
	UpdateNodeContent(ctx context.Context, id string, content json.RawMessage) error
	SetStatus(ctx context.Context, id string, s model.Status, reason string) error
	DeleteNode(ctx context.Context, id string) error

	Link(ctx context.Context, src, dst string, kind model.EdgeKind) error
	Unlink(ctx context.Context, src, dst string, kind model.EdgeKind) error
	Neighbors(ctx context.Context, id string, kind model.EdgeKind, dir model.Direction) ([]string, error)
	EdgesFor(ctx context.Context, id string) ([]model.Edge, error)

	ListFiles(ctx context.Context, nodeID string) ([]model.FileRecord, error)
	ReplaceFiles(ctx context.Context, nodeID string, files []model.FileRecord) error
	RecomputeAndStoreHash(ctx context.Context, nodeID string, cause string) (hash string, err error)

	StartRun(ctx context.Context, nodeID string, kind model.RunKind) (runID int64, err error)
	FinishRun(ctx context.Context, runID int64, r model.RunResult) error
	GetLatestRun(ctx context.Context, nodeID string) (*model.Run, error)
	RecordInputSnapshots(ctx context.Context, runID int64, observer string, snaps []model.InputSnapshot) error

	StaleNodes(ctx context.Context) ([]string, error)

	Close() error
}
