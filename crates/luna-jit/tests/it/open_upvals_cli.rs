//! 5.1: an open upvalue no closure holds is dead once a cycle's marking
//! ends. Until the sweep reaches its thread, a new closure over the
//! variable takes it back, and a function returning frees it as it closes
//! it (a closure made later over the same stack slot makes a new one). The
//! `luna` command counts as `lua.c` does, so the counts read inside the
//! cycles, which run as explicit steps, are PUC's; what PUC 5.1.5 printed
//! for each program, run as a file of that name, is below it.

use std::process::Command;

/// The output of `luna --lua=5.1 [flag] <name>` run on `src` saved as
/// `name` in a directory of its own.
fn luna(name: &str, src: &str, flag: Option<&str>) -> String {
    let dir = std::env::temp_dir().join(format!("luna-open-upvals-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    std::fs::write(dir.join(name), src).expect("the program");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_luna"));
    cmd.current_dir(&dir).arg("--lua=5.1").args(flag).arg(name);
    let out = cmd.output().expect("the luna command runs");
    let _ = std::fs::remove_dir_all(&dir); // a leftover temp directory is harmless
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const BACK: &str = r#"-- (printed as the change since the start, what the host made aside)
local out = {}
collectgarbage()
collectgarbage("stop")
local base = collectgarbage("count")
local function note(tag) out[#out + 1] = string.format("%s %.0f", tag, (collectgarbage("count") - base) * 1024) end
local x = 0
local sum = 0
for i = 1, 120 do
  local f = function() return x end
  f = nil
  collectgarbage("step", 0); collectgarbage("stop")
  local g = function() return x + i end
  note(i)
  sum = sum + g()
end
note("after loop " .. sum)
note("end")
for _, l in ipairs(out) do print(l) end
"#;

const BACK_PUC: &[&str] = &[
    "1 1136\n2 1353\n3 1570\n4 1803\n5 2004\n6 2269\n7 2470\n8 2671\n",
    "9 2664\n10 1793\n11 1996\n12 2199\n13 2402\n14 2605\n15 2808\n16 3011\n",
    "17 3214\n18 3673\n19 3876\n20 3918\n21 2402\n22 2605\n23 2808\n24 3011\n",
    "25 3214\n26 3417\n27 3620\n28 3823\n29 4026\n30 4229\n31 6399\n32 6467\n",
    "33 5005\n34 5720\n35 5923\n36 6126\n37 6329\n38 6532\n39 6735\n40 6938\n",
    "41 7141\n42 7344\n43 7439\n44 7480\n45 5901\n46 6104\n47 6307\n48 6510\n",
    "49 6713\n50 6916\n51 7119\n52 7322\n53 7525\n54 7728\n55 7823\n56 7864\n",
    "57 6285\n58 6488\n59 6691\n60 6894\n61 7097\n62 7300\n63 7503\n64 7706\n",
    "65 7909\n66 9136\n67 9285\n68 9353\n69 9475\n70 7896\n71 8099\n72 8302\n",
    "73 8505\n74 8708\n75 8911\n76 9114\n77 9317\n78 9520\n79 9723\n80 9872\n",
    "81 9940\n82 10089\n83 8313\n84 8516\n85 8719\n86 8922\n87 9125\n88 9328\n",
    "89 9531\n90 9734\n91 9937\n92 10140\n93 10263\n94 10305\n95 10455\n96 8733\n",
    "97 8936\n98 9139\n99 9342\n100 9545\n101 9750\n102 9955\n103 10160\n104 10366\n",
    "105 10572\n106 10751\n107 10847\n108 10887\n109 9167\n110 9372\n111 9577\n112 9782\n",
    "113 9987\n114 10192\n115 10398\n116 10604\n117 10810\n118 11016\n119 11222\n120 11316\n",
    "after loop 7260 11447\nend 11493\n",
];

const RETURNS: &str = r#"local out = {}
collectgarbage()
collectgarbage("stop")
local base = collectgarbage("count")
local function note(tag) out[#out + 1] = string.format("%s %.0f", tag, (collectgarbage("count") - base) * 1024) end
local function h(n)
  local y = n
  local f = function() return y end
  f = nil
  for _ = 1, 3 do collectgarbage("step", 0); collectgarbage("stop") end
  return y
end
local function k(n)
  local z = n
  local g = function() return z end
  return g()
end
for i = 1, 40 do
  h(i)
  note("h")
  k(i)
  note("k")
end
for _, l in ipairs(out) do print(l) end
"#;

const RETURNS_PUC: &[&str] = &[
    "h 1120\nk 1255\nh 1390\nk 1541\nh 1620\nk 1803\nh 1522\nk 1641\n",
    "h 1760\nk 2007\nh 2126\nk 2245\nh 1660\nk 1779\nh 1898\nk 2017\n",
    "h 2136\nk 2511\nh 2630\nk 2749\nh 2164\nk 2283\nh 2402\nk 2521\n",
    "h 2640\nk 2759\nh 2838\nk 2957\nh 2588\nk 2707\nh 2826\nk 2945\n",
    "h 3064\nk 3695\nh 3110\nk 3229\nh 3348\nk 3467\nh 5634\nk 5753\n",
    "h 5872\nk 5991\nh 6070\nk 6189\nh 5644\nk 5763\nh 5882\nk 6001\n",
    "h 6120\nk 6239\nh 6318\nk 6437\nh 5892\nk 6011\nh 6130\nk 6249\n",
    "h 6368\nk 6487\nh 6566\nk 6685\nh 6140\nk 6259\nh 6378\nk 6497\n",
    "h 6616\nk 7759\nh 7878\nk 7997\nh 8076\nk 8195\nh 7474\nk 7593\n",
    "h 7712\nk 7831\nh 7950\nk 8069\nh 8148\nk 8267\nh 7722\nk 7841\n",
];

#[test]
fn a_new_closure_takes_back_a_dead_open_upvalue() {
    let want = BACK_PUC.concat();
    assert_eq!(luna("back.lua", BACK, Some("--no-jit")), want);
    assert_eq!(luna("back.lua", BACK, None), want);
}

#[test]
fn a_return_frees_a_dead_open_upvalue_before_the_sweep() {
    let want = RETURNS_PUC.concat();
    assert_eq!(luna("ret.lua", RETURNS, Some("--no-jit")), want);
    assert_eq!(luna("ret.lua", RETURNS, None), want);
}
