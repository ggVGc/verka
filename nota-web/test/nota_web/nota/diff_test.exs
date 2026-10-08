defmodule NotaWeb.Nota.DiffTest do
  use ExUnit.Case, async: true

  alias NotaWeb.Nota.Diff

  @modified "diff --git a/lib/demo.ex b/lib/demo.ex\nindex 1111111..2222222 100644\n--- a/lib/demo.ex\n+++ b/lib/demo.ex\n@@ -1,3 +1,4 @@ defmodule Demo do\n defmodule Demo do\n-  def old, do: 1\n+  def new, do: 2\n+  def extra, do: 3\n end\n"

  @added "diff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0000000..1111111\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\n"

  test "parses a modified file with numbered lines" do
    assert [file] = Diff.parse(@modified)
    assert file.path == "lib/demo.ex"
    assert file.old_path == "lib/demo.ex"
    assert file.status == :modified
    assert file.additions == 2
    assert file.deletions == 1

    assert [hunk] = file.hunks
    assert hunk.header =~ "@@ -1,3 +1,4 @@"
    assert Enum.map(hunk.lines, & &1.kind) == [:ctx, :del, :add, :add, :ctx]
    assert Enum.map(hunk.lines, & &1.old_no) == [1, 2, nil, nil, 3]
    assert Enum.map(hunk.lines, & &1.new_no) == [1, nil, 2, 3, 4]
  end

  test "detects files added over /dev/null" do
    assert [file] = Diff.parse(@added)
    assert file.path == "new.txt"
    assert file.old_path == nil
    assert file.status == :added
    assert file.additions == 2
    assert file.deletions == 0
  end

  test "marks binary patches" do
    patch =
      "diff --git a/logo.png b/logo.png\nindex 1111111..2222222 100644\nBinary files a/logo.png and b/logo.png differ\n"

    assert [file] = Diff.parse(patch)
    assert file.binary
    assert file.path == "logo.png"
  end
end
