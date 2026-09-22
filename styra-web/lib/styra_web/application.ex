defmodule StyraWeb.Application do
  # See https://elixir.hexdocs.pm/Application.html
  # for more information on OTP Applications
  @moduledoc false

  use Application

  require Logger

  @impl true
  def start(_type, _args) do
    log_styra_socket()

    children = [
      StyraWebWeb.Telemetry,
      {DNSCluster, query: Application.get_env(:styra_web, :dns_cluster_query) || :ignore},
      {Phoenix.PubSub, name: StyraWeb.PubSub},
      # Start a worker by calling: StyraWeb.Worker.start_link(arg)
      # {StyraWeb.Worker, arg},
      # Start to serve requests, typically the last entry
      StyraWebWeb.Endpoint
    ]

    # See https://elixir.hexdocs.pm/Supervisor.html
    # for other strategies and supported options
    opts = [strategy: :one_for_one, name: StyraWeb.Supervisor]
    Supervisor.start_link(children, opts)
  end

  defp log_styra_socket do
    case StyraWeb.StyraAPI.socket_path() do
      {:ok, path} -> Logger.info("Styra socket: #{path}")
      {:error, message} -> Logger.warning("Styra socket is not configured: #{message}")
    end
  end

  # Tell Phoenix to update the endpoint configuration
  # whenever the application is updated.
  @impl true
  def config_change(changed, _new, removed) do
    StyraWebWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
