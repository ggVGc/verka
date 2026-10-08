defmodule NotaWeb.Nota.Cli do
  @moduledoc """
  Runs the `nota` executable and decodes its JSON.

  The binary is `$NOTA_BIN`, then the `:nota_binary` application setting, then
  `nota` on `PATH`. JSON is decoded with string keys: the viewer never builds
  atoms from repository data.
  """

  @spec binary() :: String.t()
  def binary do
    Application.get_env(:nota_web, :nota_binary) || System.get_env("NOTA_BIN") || "nota"
  end

  @spec list(String.t()) :: {:ok, map()} | {:error, String.t()}
  def list(repository) do
    run(["list", "--repository", repository, "--json"])
  end

  @spec show(String.t(), String.t()) :: {:ok, map()} | {:error, String.t()}
  def show(repository, branch) do
    run(["show", "--repository", repository, "--branch", branch, "--json"])
  end

  defp run(args) do
    binary = binary()

    case System.cmd(binary, args, stderr_to_stdout: false) do
      {output, 0} ->
        Jason.decode(output)

      {_output, code} ->
        {:error, "`#{binary} #{Enum.join(args, " ")}` exited with status #{code}"}
    end
  rescue
    ErlangError ->
      {:error, "could not run the `#{binary()}` executable; build it or set NOTA_BIN to its path"}
  end
end
