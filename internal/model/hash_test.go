package model

import "testing"

func TestCanonicalizeSortsMapKeys(t *testing.T) {
	inputs := [][]byte{
		[]byte(`{"b":1,"a":2,"c":{"z":9,"y":8}}`),
		[]byte(`{"c":{"y":8,"z":9},"a":2,"b":1}`),
		[]byte(` { "a" : 2 , "b" : 1 , "c" : { "z" : 9 , "y" : 8 } } `),
	}
	want := `{"a":2,"b":1,"c":{"y":8,"z":9}}`
	for _, in := range inputs {
		got, err := Canonicalize(in)
		if err != nil {
			t.Fatalf("Canonicalize(%q) error: %v", string(in), err)
		}
		if string(got) != want {
			t.Errorf("Canonicalize(%q) = %q, want %q", string(in), string(got), want)
		}
	}
}

func TestCanonicalizePreservesNumbers(t *testing.T) {
	in := []byte(`{"n":1.0000000000000001}`)
	got, err := Canonicalize(in)
	if err != nil {
		t.Fatal(err)
	}
	want := `{"n":1.0000000000000001}`
	if string(got) != want {
		t.Errorf("lost precision: got %s, want %s", got, want)
	}
}

func TestContentHashIsDeterministic(t *testing.T) {
	a, err := ComputeContentHash([]byte(`{"a":1,"b":[1,2,3]}`), []FileRecord{
		{RelPath: "b.txt", SHA256: "bb"},
		{RelPath: "a.txt", SHA256: "aa"},
	})
	if err != nil {
		t.Fatal(err)
	}
	b, err := ComputeContentHash([]byte(`{"b":[1,2,3],"a":1}`), []FileRecord{
		{RelPath: "a.txt", SHA256: "aa"},
		{RelPath: "b.txt", SHA256: "bb"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if a != b {
		t.Fatalf("expected identical hashes, got %s vs %s", a, b)
	}
}

func TestContentHashChangesOnContentChange(t *testing.T) {
	a, _ := ComputeContentHash([]byte(`{"a":1}`), nil)
	b, _ := ComputeContentHash([]byte(`{"a":2}`), nil)
	if a == b {
		t.Fatal("expected different hashes for different content")
	}
}

func TestContentHashChangesOnFileChange(t *testing.T) {
	a, _ := ComputeContentHash([]byte(`{"a":1}`), []FileRecord{{RelPath: "x", SHA256: "h1"}})
	b, _ := ComputeContentHash([]byte(`{"a":1}`), []FileRecord{{RelPath: "x", SHA256: "h2"}})
	if a == b {
		t.Fatal("expected different hashes when file hash changes")
	}
}

func TestContentHashIgnoresStatus(t *testing.T) {
	// Status is a field on Node but is not passed to ComputeContentHash. This
	// test just documents the contract: ComputeContentHash depends only on
	// (content, files) and nothing else.
	a, _ := ComputeContentHash([]byte(`{"a":1}`), nil)
	b, _ := ComputeContentHash([]byte(`{"a":1}`), nil)
	if a != b {
		t.Fatal("expected identical hashes for identical inputs")
	}
}
