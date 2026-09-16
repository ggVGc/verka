  # Hand-written support the generated tables are useless without: a validator
  # driven by `types/0`, and the two halves of a request/response exchange.
  # Everything here is generic over the descriptors; it says nothing about any
  # particular operation, so it does not need regenerating when the protocol
  # gains one.
  #
  # Elixir needs no null sentinel, which the Lua library does: a map holds nil
  # as a value and `Map.fetch/2` still tells an absent key from one set to it.
  # So the distinction the protocol cares about — clearing a Session name is
  # not the same as not mentioning it — is said the obvious way, by leaving the
  # key out or setting it to nil.

  @doc ~S"""
  Check a value against a named wire type.

  Returns `:ok`, or `{:error, message}` with a message naming the field that
  was wrong. Works on anything in `types/0`: an `InteractionUpdate` off the
  update stream as readily as a request.

      :ok = Styra.Protocol.validate("LaunchPolicy", %{network: true})
  """
  @spec validate(String.t(), term()) :: :ok | {:error, String.t()}
  def validate(name, value) do
    case Map.fetch(types(), name) do
      :error -> {:error, ~s(unknown wire type "#{name}")}
      {:ok, _descriptor} -> check_value(%{kind: :ref, name: name}, value, "")
    end
  end

  @doc ~S"""
  Build the request for `operation` from `data`, checking it first.

  Returns `{:ok, request}` or `{:error, message}`. The request is a plain map
  with string keys, for your own encoder to serialise and your own socket to
  carry — one JSON object per line.

  The check is worth having because `Request` denies unknown fields: a client
  that sends a misspelled key gets a socket round trip and an error string
  back instead of an answer, far from the line that made the mistake.

      {:ok, request} = Styra.Protocol.build("rename_session", %{id: "styra-1", name: nil})
  """
  @spec build(String.t(), term()) :: {:ok, map()} | {:error, String.t()}
  def build(operation, data \\ :none)

  def build(operation, data) do
    descriptor = Map.fetch!(types(), "Request")

    case variant_named(descriptor, operation) do
      nil ->
        {:error, ~s("#{operation}" is not a Styra operation)}

      %{payload: %{kind: :unit}} ->
        if data == :none do
          {:ok, %{"operation" => operation}}
        else
          {:error, "#{operation} takes no data"}
        end

      variant ->
        cond do
          data == :none ->
            {:error, "#{operation} needs its data"}

          true ->
            case check_payload(variant, {:given, data}, "data", deny?(descriptor)) do
              :ok -> {:ok, %{"operation" => operation, "data" => data}}
              {:error, message} -> {:error, "#{operation}: #{message}"}
            end
        end
    end
  end

  @doc ~S"""
  `build/2`, raising `ArgumentError` on a request the server would refuse.
  """
  @spec build!(String.t(), term()) :: map()
  def build!(operation, data \\ :none) do
    case build(operation, data) do
      {:ok, request} -> request
      {:error, message} -> raise ArgumentError, message
    end
  end

  @doc ~S"""
  Read a decoded `WireResponse`: `{:ok, response}` carrying the response map
  (its `"type"` and `"data"`), or `{:error, message}` with the server's own
  error.

      {:ok, response} = Styra.Protocol.unwrap(Jason.decode!(line))
  """
  @spec unwrap(term()) :: {:ok, term()} | {:error, String.t()}
  def unwrap(wire) when not is_map(wire) do
    {:error, "the server's reply is not an object (#{kind_of(wire)})"}
  end

  def unwrap(wire) do
    case keyed(wire) do
      %{"status" => "ok", "response" => response} ->
        {:ok, response}

      %{"status" => "ok"} ->
        {:error, "the server's reply is ok but carries no response"}

      %{"status" => "error", "error" => message} when is_binary(message) ->
        {:error, message}

      %{"status" => "error"} ->
        {:error, "the server reported an error with no message"}

      %{"status" => status} ->
        {:error, "the server's reply has an unknown status (#{inspect(status)})"}

      _ ->
        {:error, "the server's reply has no status"}
    end
  end

  @doc ~S"""
  The data of a response of the expected type, or `{:error, message}`. Saves
  every caller the same two checks: that the request succeeded, and that the
  reply is about what was asked.

      {:ok, health} = Styra.Protocol.expect(reply, Styra.Protocol.Response.health())
  """
  @spec expect(term(), String.t()) :: {:ok, term()} | {:error, String.t()}
  def expect(reply, kind) do
    with {:ok, response} <- unwrap(reply) do
      case is_map(response) && keyed(response) do
        %{"type" => ^kind} = response -> {:ok, Map.get(response, "data")}
        %{"type" => other} -> {:error, ~s(expected a "#{kind}" response, got "#{other}")}
        _ -> {:error, "the server's response names no type"}
      end
    end
  end

  # Checking ----------------------------------------------------------------

  defp check_value(%{kind: :optional, inner: inner}, value, path) do
    if is_nil(value), do: :ok, else: check_value(inner, value, path)
  end

  defp check_value(_shape, nil, path), do: fail(path, "is not nullable")

  defp check_value(%{kind: :any}, _value, _path), do: :ok

  defp check_value(%{kind: :string}, value, path) do
    if is_binary(value), do: :ok, else: fail(path, "expected a string, got #{kind_of(value)}")
  end

  defp check_value(%{kind: :number} = shape, value, path) do
    cond do
      not is_number(value) ->
        fail(path, "expected a number, got #{kind_of(value)}")

      Map.get(shape, :integer, false) && not is_integer(value) ->
        fail(path, "expected a whole number, got #{inspect(value)}")

      true ->
        :ok
    end
  end

  defp check_value(%{kind: :boolean}, value, path) do
    if is_boolean(value), do: :ok, else: fail(path, "expected a boolean, got #{kind_of(value)}")
  end

  defp check_value(%{kind: :list, item: item}, value, path) when is_list(value) do
    value
    |> Enum.with_index()
    |> reduce_ok(fn {element, index} -> check_value(item, element, path_of(path, index)) end)
  end

  defp check_value(%{kind: :list}, value, path) do
    fail(path, "expected a list, got #{kind_of(value)}")
  end

  defp check_value(%{kind: :map, value: shape}, value, path) when is_map(value) do
    reduce_ok(value, fn {key, item} ->
      if is_binary(key) or is_atom(key) do
        check_value(shape, item, path_of(path, key))
      else
        fail(path, "has a non-string key")
      end
    end)
  end

  defp check_value(%{kind: :map}, value, path) do
    fail(path, "expected a map, got #{kind_of(value)}")
  end

  defp check_value(%{kind: :ref, name: name}, value, path) do
    case Map.fetch(types(), name) do
      :error ->
        fail(path, ~s(refers to unknown wire type "#{name}"))

      {:ok, %{kind: :struct} = descriptor} ->
        check_fields(descriptor.fields, value, path, deny?(descriptor))

      {:ok, descriptor} ->
        check_enum(descriptor, value, path)
    end
  end

  defp check_value(shape, _value, path) do
    fail(path, "has no shape the generator understands (#{inspect(shape)})")
  end

  # `reserved` names keys that belong to the encoding rather than to the fields
  # (an internal tag sitting alongside them), so they do not read as unknown.
  defp check_fields(fields, value, path, deny_unknown, reserved \\ [])

  defp check_fields(_fields, value, path, _deny_unknown, _reserved) when not is_map(value) do
    fail(path, "expected a map, got #{kind_of(value)}")
  end

  defp check_fields(fields, value, path, deny_unknown, reserved) do
    given = keyed(value)

    with :ok <- reduce_ok(fields, &check_field(&1, given, path)) do
      if deny_unknown do
        known = MapSet.new(Enum.map(fields, & &1.name) ++ reserved)

        case Enum.find(Map.keys(given), &(not MapSet.member?(known, &1))) do
          nil -> :ok
          unknown -> fail(path, ~s(unknown field "#{unknown}"))
        end
      else
        :ok
      end
    end
  end

  defp check_field(field, given, path) do
    case Map.fetch(given, field.name) do
      :error ->
        if field.required do
          fail(path, ~s(missing required field "#{field.name}"))
        else
          :ok
        end

      {:ok, value} ->
        check_value(field.type, value, path_of(path, field.name))
    end
  end

  # `content` is `:absent` or `{:given, value}`, because nil is a value here
  # and not an absence.
  defp check_payload(%{payload: %{kind: :unit}} = variant, content, path, _deny_unknown) do
    if content == :absent, do: :ok, else: fail(path, ~s("#{variant.name}" carries no data))
  end

  defp check_payload(variant, :absent, path, _deny_unknown) do
    fail(path, ~s("#{variant.name}" needs its data))
  end

  defp check_payload(%{payload: %{kind: :newtype, type: type}}, {:given, content}, path, _deny) do
    check_value(type, content, path)
  end

  defp check_payload(%{payload: %{kind: :tuple, items: items}}, {:given, content}, path, _deny) do
    if is_list(content) and length(content) == length(items) do
      items
      |> Enum.zip(content)
      |> Enum.with_index()
      |> reduce_ok(fn {{item, value}, index} -> check_value(item, value, path_of(path, index)) end)
    else
      fail(path, "expected a list of #{length(items)} values, got #{kind_of(content)}")
    end
  end

  defp check_payload(%{payload: %{kind: :struct} = payload}, {:given, content}, path, deny) do
    check_fields(payload.fields, content, path, Map.get(payload, :deny_unknown_fields, false) || deny)
  end

  # Dispatched on a value rather than by pattern, deliberately. `@types` is a
  # literal, so the compiler knows exactly which tagging styles this protocol
  # uses and reports a clause for any it does not as dead code — which would
  # make a generic runtime warn on every compile, and warn *differently* as
  # the protocol gains and loses shapes. The styles are a closed set that the
  # generator understands whether or not today's protocol uses all of them.
  defp check_enum(descriptor, value, path) do
    style = descriptor.tagging.style

    cond do
      style == :untagged -> :ok
      Map.get(descriptor, :plain, false) -> check_plain(descriptor, value, path)
      style == :external -> check_external(descriptor, value, path)
      true -> check_by_tag(descriptor, value, path)
    end
  end

  defp check_plain(descriptor, value, path) do
    cond do
      not is_binary(value) ->
        fail(path, "expected one of #{spellings_of(descriptor)}, got #{kind_of(value)}")

      is_nil(variant_named(descriptor, value)) ->
        fail(path, ~s("#{value}" is not one of #{spellings_of(descriptor)}))

      true ->
        :ok
    end
  end

  defp check_external(descriptor, value, path) when is_binary(value) do
    case variant_named(descriptor, value) do
      nil -> fail(path, ~s("#{value}" is not one of #{spellings_of(descriptor)}))
      variant -> check_payload(variant, :absent, path, deny?(descriptor))
    end
  end

  defp check_external(descriptor, value, path) do
    cond do
      not is_map(value) ->
        fail(path, "expected a map or a string, got #{kind_of(value)}")

      map_size(value) == 0 ->
        fail(path, "names no variant; expected one of #{spellings_of(descriptor)}")

      map_size(value) > 1 ->
        fail(path, "names more than one variant")

      true ->
        [{name, content}] = Map.to_list(keyed(value))

        case variant_named(descriptor, name) do
          nil ->
            fail(path, ~s("#{name}" is not one of #{spellings_of(descriptor)}))

          variant ->
            check_payload(variant, {:given, content}, path_of(path, name), deny?(descriptor))
        end
    end
  end

  defp check_by_tag(descriptor, value, path) when is_map(value) do
    given = keyed(value)
    tag = descriptor.tagging.tag

    case Map.fetch(given, tag) do
      {:ok, name} when is_binary(name) ->
        case variant_named(descriptor, name) do
          nil -> fail(path, ~s("#{name}" is not one of #{spellings_of(descriptor)}))
          variant -> check_tagged(descriptor, variant, given, path)
        end

      _ ->
        fail(path, ~s(has no "#{tag}" naming one of #{spellings_of(descriptor)}))
    end
  end

  defp check_by_tag(_descriptor, value, path) do
    fail(path, "expected a map, got #{kind_of(value)}")
  end

  defp check_tagged(descriptor, variant, given, path) do
    if descriptor.tagging.style == :adjacent do
      check_adjacent(descriptor, variant, given, path)
    else
      check_internal(descriptor, variant, given, path)
    end
  end

  defp check_adjacent(descriptor, variant, given, path) do
    tagging = descriptor.tagging

    content =
      case Map.fetch(given, tagging.content) do
        {:ok, value} -> {:given, value}
        :error -> :absent
      end

    with :ok <- check_payload(variant, content, path_of(path, tagging.content), deny?(descriptor)) do
      if deny?(descriptor) do
        extra = Enum.find(Map.keys(given), &(&1 != tagging.tag and &1 != tagging.content))
        if extra, do: fail(path, ~s(unknown field "#{extra}")), else: :ok
      else
        :ok
      end
    end
  end

  # Internally tagged: the payload's fields sit beside the tag.
  defp check_internal(descriptor, variant, given, path) do
    tagging = descriptor.tagging

    case variant.payload do
      %{kind: :newtype, type: type} ->
        check_value(type, given, path)

      %{kind: :struct, fields: fields} ->
        check_fields(fields, given, path, deny?(descriptor), [tagging.tag])

      _ ->
        check_fields([], given, path, deny?(descriptor), [tagging.tag])
    end
  end

  # Odds and ends -----------------------------------------------------------

  defp variant_named(descriptor, name) do
    Enum.find(descriptor.variants, &(&1.name == name))
  end

  defp spellings_of(descriptor) do
    descriptor.variants |> Enum.map(&~s("#{&1.name}")) |> Enum.join(", ")
  end

  defp deny?(descriptor), do: Map.get(descriptor, :deny_unknown_fields, false)

  # A map decoded from JSON has string keys; one written by hand in Elixir
  # usually has atom ones. Both are read, because a client that has to spell
  # its own request in string keys is being made to work for the generator.
  defp keyed(map) do
    Map.new(map, fn {key, value} -> {to_string(key), value} end)
  end

  defp path_of("", key), do: to_string(key)
  defp path_of(path, key), do: path <> "." <> to_string(key)

  defp fail("", message), do: {:error, message}
  defp fail(path, message), do: {:error, path <> ": " <> message}

  defp reduce_ok(items, check) do
    Enum.reduce_while(items, :ok, fn item, :ok ->
      case check.(item) do
        :ok -> {:cont, :ok}
        {:error, _} = error -> {:halt, error}
      end
    end)
  end

  defp kind_of(nil), do: "nil"
  defp kind_of(value) when is_binary(value), do: "a string"
  defp kind_of(value) when is_boolean(value), do: "a boolean"
  defp kind_of(value) when is_number(value), do: "a number"
  defp kind_of(value) when is_list(value), do: "a list"
  defp kind_of(value) when is_map(value), do: "a map"
  defp kind_of(value) when is_atom(value), do: "an atom"
  defp kind_of(value), do: inspect(value)
