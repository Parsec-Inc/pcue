-- Helpers compartilhados: paleta, canvas de caracteres e gerador de .aseprite.
local M = {}

M.PAL = {
  K = "#cbd5e1", -- contorno claro
  P = "#0b1020", -- pupila
  D = "#2f3b5c", -- corpo
  C = "#22d3ee", -- ciano (accent)
  c = "#0e7490", -- ciano escuro
  B = "#67e8f9", -- ciano claro
  W = "#f1f5f9", -- branco
  L = "#94a3b8", -- cinza
  G = "#22c55e", -- verde
  g = "#86efac",
  R = "#ef4444", -- vermelho
  r = "#7f1d1d",
  S = "#f87171",
  A = "#f59e0b", -- ambar
  a = "#b45309",
  Y = "#facc15", -- amarelo
  y = "#fef08a",
  N = "#1e293b", -- fundo do relogio
}

local function hex(h)
  local r, g, b = h:match("#(%x%x)(%x%x)(%x%x)")
  return app.pixelColor.rgba(tonumber(r, 16), tonumber(g, 16), tonumber(b, 16), 255)
end

function M.canvas(w, h)
  local c = { w = w, h = h }
  for y = 1, h do
    c[y] = {}
    for x = 1, w do c[y][x] = "." end
  end
  return c
end

function M.set(c, x, y, ch)
  if x >= 0 and y >= 0 and x < c.w and y < c.h then c[y + 1][x + 1] = ch end
end

-- Copia `rows` (strings) para o canvas; '.' e transparente.
function M.blit(c, rows, x0, y0)
  for j, row in ipairs(rows) do
    for i = 1, #row do
      local ch = row:sub(i, i)
      if ch ~= "." then M.set(c, x0 + i - 1, y0 + j - 1, ch) end
    end
  end
end

function M.rows(c)
  local out = {}
  for y = 1, c.h do out[y] = table.concat(c[y]) end
  return out
end

-- frames = { {rows = {...}, dur = ms}, ... }
function M.build(name, w, h, frames)
  local dir = app.params.dir
  local spr = Sprite(w, h, ColorMode.RGB)
  local layer = spr.layers[1]
  layer.name = "art"
  for i, f in ipairs(frames) do
    assert(#f.rows == h, name .. " frame " .. i .. ": altura " .. #f.rows)
    if i > 1 then spr:newEmptyFrame() end
    spr.frames[i].duration = f.dur / 1000
    local img = Image(w, h, ColorMode.RGB)
    for y = 1, h do
      assert(#f.rows[y] == w, name .. " frame " .. i .. " linha " .. y .. ": largura " .. #f.rows[y])
      for x = 1, w do
        local ch = f.rows[y]:sub(x, x)
        if ch ~= "." then
          local col = M.PAL[ch]
          assert(col, "cor desconhecida '" .. ch .. "' em " .. name)
          img:putPixel(x - 1, y - 1, hex(col))
        end
      end
    end
    spr:newCel(layer, spr.frames[i], img, Point(0, 0))
  end
  spr:saveAs(dir .. "/" .. name .. ".aseprite")
  spr:close()
end

return M
