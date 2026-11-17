-- Mascote do pcue: claquete com olhos. Gera mascot, mascot_sleep e mascot_cheer.
local U = dofile(app.params.src .. "/common.lua")

local BAR = {
  ".KKKKKKKKKKKKKK.",
  ".KCCDDCCDDCCDDK.",
  ".KDDCCDDCCDDCCK.",
  ".KKKKKKKKKKKKKK.",
}
local TOP, BOT = ".KKKKKKKKKKKKKK.", ".KKKKKKKKKKKKKK."
local FILL = ".KDDDDDDDDDDDDK."
local FEET = { ".....KK....KK...", "....KKK....KKK.." }

-- linhas do rosto (8 linhas de corpo): topo, folga, olhos x2, bochechas, boca x2, base
local FACES = {
  normal = { TOP, FILL, ".KDDWWDDDDWWDDK.", ".KDDWPDDDDWPDDK.", ".KDCDDDDDDDDCDK.",
             ".KDDDDCDDCDDDDK.", ".KDDDDDCCDDDDDK.", BOT },
  blink  = { TOP, FILL, FILL, ".KDDWWDDDDWWDDK.", ".KDCDDDDDDDDCDK.",
             ".KDDDDCDDCDDDDK.", ".KDDDDDCCDDDDDK.", BOT },
  happy  = { TOP, FILL, ".KDDDWDDDDDWDDK.", ".KDDWDWDDDWDWDK.", ".KDCDDDDDDDDCDK.",
             ".KDDDCCCCCCDDDK.", ".KDDDDCCCCDDDDK.", BOT },
}

-- Barra levantada: cada coluna sobe `shift(c)` linhas (dobradica a esquerda).
local function draw_bar(cv, y0, shift)
  for c = 1, 14 do
    local s = shift(c)
    for j = 1, #BAR do
      local ch = BAR[j]:sub(c + 1, c + 1)
      U.set(cv, c, y0 + j - 1 - s, ch)
    end
  end
  if shift(14) > 0 then -- preenche vao sob a barra levantada
    for c = 1, 14 do
      for yy = y0 + #BAR - shift(c), y0 + #BAR - 1 do
        if cv[yy + 1] and cv[yy + 1][c + 1] == "." then
          -- vazio mantido de proposito: barra aberta deixa o ceu a mostra
        end
      end
    end
  end
end

local function sparks(cv, list, ch)
  for _, p in ipairs(list) do U.set(cv, p[1], p[2], ch or "y") end
end

local function render(o)
  local cv = U.canvas(16, 16)
  local dy = o.dy or 0
  draw_bar(cv, 2 + dy, o.shift or function() return 0 end)
  U.blit(cv, FACES[o.face or "normal"], 0, 6 + dy)
  if o.feet ~= false then U.blit(cv, FEET, 0, 14 + dy) end
  if o.sparks then sparks(cv, o.sparks, o.spark_ch) end
  return U.rows(cv)
end

local open_hi = function(c) return math.floor((c - 1) / 4.5) end   -- ate 2 linhas
local open_lo = function(c) return math.floor((c - 1) / 7) end     -- ate 1 linha

-- mascot (idle, ~2.5s)
U.build("mascot", 16, 16, {
  { rows = render{}, dur = 900 },
  { rows = render{ face = "blink" }, dur = 120 },
  { rows = render{}, dur = 700 },
  { rows = render{ shift = open_hi }, dur = 350 },
  { rows = render{ sparks = { {0, 5}, {15, 5}, {0, 3}, {15, 3} } }, dur = 120 },
  { rows = render{}, dur = 310 },
})

-- mascot_sleep: encolhido, olhos fechados, "z" subindo
local SLEEP_BODY = {
  TOP, FILL, ".KDDWWDDDDWWDDK.", ".KDCDDDDDDDDCDK.", ".KDDDDDCCDDDDDK.", BOT,
}
local Z1 = { "BB", "B." }
local Z2 = { "BBBB", "..B.", ".B..", "BBBB" }
local function sleep_frame(z)
  local cv = U.canvas(16, 16)
  draw_bar(cv, 6, function() return 0 end)
  U.blit(cv, SLEEP_BODY, 0, 10)
  U.blit(cv, z.small and Z1 or Z2, z.x, z.y)
  return U.rows(cv)
end
U.build("mascot_sleep", 16, 16, {
  { rows = sleep_frame{ x = 12, y = 4, small = true }, dur = 400 },
  { rows = sleep_frame{ x = 11, y = 2 }, dur = 400 },
  { rows = sleep_frame{ x = 11, y = 1 }, dur = 400 },
  { rows = sleep_frame{ x = 11, y = 0 }, dur = 400 },
})

-- mascot_cheer: pulinho feliz
U.build("mascot_cheer", 16, 16, {
  { rows = render{ face = "happy" }, dur = 120 },
  { rows = render{ face = "happy", dy = -1, shift = open_lo,
      sparks = { {0, 4}, {15, 6} } }, dur = 120 },
  { rows = render{ face = "happy", dy = -1, shift = open_lo,
      sparks = { {0, 7}, {15, 3}, {1, 1}, {14, 8} }, spark_ch = "W" }, dur = 120 },
  { rows = render{ face = "happy", sparks = { {0, 5}, {15, 5} } }, dur = 120 },
})
