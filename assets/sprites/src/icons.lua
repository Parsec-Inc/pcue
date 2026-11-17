-- Icones 8x8 de estado do pcue.
local U = dofile(app.params.src .. "/common.lua")

local function sub(rows, map)
  local out = {}
  for i, r in ipairs(rows) do out[i] = r:gsub(".", function(ch) return map[ch] or ch end) end
  return out
end

-- rec: disco vermelho pulsando
local DISC = { "..RRRR..", ".RSRRRR.", "RSRRRRRR", "RRRRRRRR", "RRRRRRRR", "RRRRRRRR", ".RRRRRR.", "..RRRR.." }
local SMALL = { "........", "..RRRR..", ".RSRRRR.", ".RRRRRR.", ".RRRRRR.", ".RRRRRR.", "..RRRR..", "........" }
U.build("rec", 8, 8, {
  { rows = DISC, dur = 450 },
  { rows = SMALL, dur = 100 },
  { rows = sub(SMALL, { R = "r", S = "r" }), dur = 350 },
  { rows = SMALL, dur = 100 },
})

-- paused: duas barras ambar
local PAUSE = { "........", ".AA..AA.", ".AA..AA.", ".AA..AA.", ".AA..AA.", ".AA..AA.", ".AA..AA.", "........" }
U.build("paused", 8, 8, {
  { rows = PAUSE, dur = 600 },
  { rows = sub(PAUSE, { A = "a" }), dur = 600 },
})

-- running: arco giratorio ciano sobre anel escuro
local RING = { {2,0},{3,0},{4,0},{5,0},{6,1},{7,2},{7,3},{7,4},{7,5},{6,6},{5,7},{4,7},{3,7},{2,7},{1,6},{0,5},{0,4},{0,3},{0,2},{1,1} }
local function spinner(head)
  local cv = U.canvas(8, 8)
  for _, p in ipairs(RING) do U.set(cv, p[1], p[2], "c") end
  local tones = { "W", "B", "B", "C", "C", "C", "C", "C", "C" } -- cabeca -> cauda
  for k, ch in ipairs(tones) do
    local p = RING[((head - (k - 1)) % #RING) + 1]
    U.set(cv, p[1], p[2], ch)
    local dx, dy = 3.5 - p[1], 3.5 - p[2]
    if k <= 6 then -- engrossa o arco com o pixel interno
      local sx = (math.abs(dx) >= math.abs(dy) / 2) and (dx > 0 and 1 or -1) or 0
      local sy = (math.abs(dy) >= math.abs(dx) / 2) and (dy > 0 and 1 or -1) or 0
      if cv[p[2] + sy + 1][p[1] + sx + 1] == "." then U.set(cv, p[1] + sx, p[2] + sy, ch == "W" and "B" or ch) end
    end
  end
  return U.rows(cv)
end
U.build("running", 8, 8, {
  { rows = spinner(5), dur = 120 }, { rows = spinner(10), dur = 120 },
  { rows = spinner(15), dur = 120 }, { rows = spinner(0), dur = 120 },
})

-- ok: check verde com "pop"
local CHECK = { "........", "......GG", ".....GG.", "G...GG..", "GG.GG...", ".GGG....", "..G.....", "........" }
local CHECK_S = { "........", "........", "......g.", "g....g..", "gg..g...", ".gg.....", "........", "........" }
U.build("ok", 8, 8, {
  { rows = CHECK_S, dur = 70 },
  { rows = sub(CHECK, { G = "g" }), dur = 80 },
  { rows = CHECK, dur = 1000 },
})

-- fail: X vermelho
U.build("fail", 8, 8, {
  { rows = { "RR....RR", ".RR..RR.", "..RRRR..", "...RR...", "...RR...", "..RRRR..", ".RR..RR.", "RR....RR" }, dur = 1000 },
})

-- event: raio amarelo com leve cintilar
local BOLT = { ".....YY.", "....YY..", "...YY...", "..YYYYY.", "....YY..", "...YY...", "..YY....", "..Y....." }
U.build("event", 8, 8, {
  { rows = BOLT, dur = 600 },
  { rows = { ".....yy.", "....yY..", "...yY...", "..yYYYY.", "....yY..", "...YY...", "..YY....", "..Y....." }, dur = 150 },
})

-- clock: mostrador com ponteiro girando
local FACE = { "..WWWW..", ".WNNNNW.", "WNNNNNNW", "WNNNNNNW", "WNNNNNNW", "WNNNNNNW", ".WNNNNW.", "..WWWW.." }
local function clock(rot)
  local cv = U.canvas(8, 8)
  U.blit(cv, FACE, 0, 0)
  local pts = { {3,3},{4,3},{3,4},{4,4},{4,2},{4,1} } -- centro + ponteiro para cima
  for _, p in ipairs(pts) do
    local x, y = p[1], p[2]
    for _ = 1, rot do x, y = 7 - y, x end
    U.set(cv, x, y, "C")
  end
  U.set(cv, 3, 3, "W"); U.set(cv, 4, 3, "W")
  return U.rows(cv)
end
U.build("clock", 8, 8, {
  { rows = clock(0), dur = 250 }, { rows = clock(1), dur = 250 },
  { rows = clock(2), dur = 250 }, { rows = clock(3), dur = 250 },
})

-- interval: laco de repeticao (repeat) com brilho percorrendo o caminho
local REPEAT = { "......C.", ".CCCCCCC", "C.....C.", "C.......", ".......C", ".C.....C", "CCCCCCC.", ".C......" }
local PATH = { {1,1},{2,1},{3,1},{4,1},{5,1},{7,4},{7,5},{6,6},{5,6},{4,6},{3,6},{2,6},{1,6},{0,3},{0,2} }
local function interval(head)
  local cv = U.canvas(8, 8)
  U.blit(cv, REPEAT, 0, 0)
  for k, ch in ipairs({ "W", "B", "B" }) do
    local p = PATH[((head - (k - 1)) % #PATH) + 1]
    U.set(cv, p[1], p[2], ch)
  end
  return U.rows(cv)
end
U.build("interval", 8, 8, {
  { rows = interval(0), dur = 200 }, { rows = interval(4), dur = 200 },
  { rows = interval(8), dur = 200 }, { rows = interval(12), dur = 200 },
})
