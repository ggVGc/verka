if vim.g.loaded_svara then
  return
end
vim.g.loaded_svara = true

vim.api.nvim_create_user_command("SvaraSend", function(command)
  local session_id = command.fargs[1]
  local message = table.concat(command.fargs, " ", 2)
  local sent, err = require("svara").send_message(session_id, message)
  if not sent then
    vim.notify("Svara: " .. err, vim.log.levels.ERROR)
    return
  end
  vim.notify("Svara: message sent", vim.log.levels.INFO)
end, {
  nargs = "+",
  desc = "Send a message to an existing Styra session",
})
