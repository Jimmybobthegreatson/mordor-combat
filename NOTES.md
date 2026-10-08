# mordor-combat notes

Format and rule notes for the combat, combat animation, walk / run and crouch / stealth parts of this project. Notes about climbing, rails, the HUD world and
the character meshes belong to the full private project and are left out. Sections keep the order they were written in; later ones correct earlier ones.

## Status
- **Containers**: LTAR (`.arch05`), BNDL (`.embb`), VFS over `x64/default.archcfg`, `som` CLI.
  `som verify`: 7766 bundles / 266054 records; 365 bundles in DLC2.arch05 and NF_Patch.arch05 do
  not parse (patch bundle variant). The VFS falls back to the next-lower bundle.
- **Skeleton** (`SKEL` v8): bone tree, model-space bind pose.
- **Animation** (`ANIX` v42): banks of named clips; codecs SV01, AV01, SQ03, AQ03, UATR.

## Locomotion (done)
- `som-formats::animator`: `Library` maps banks onto the skeleton, `Animator` cross-fades layers and returns a
  `Step { pose, travel, yaw }`. The `Null` bone carries root travel; it is stripped from the pose and the
  entity moves instead (travel is expressed in the clip's own start frame, yaw = `2*atan2(qy, qw)`).
- Clips used (all verified by `som clips <bank>`; names print truncated for some banks but lookup is by hash):
  idle `PL_Base_L_Fist_IdleBase` (pl_movbase), walk `PL_Cmbt_Walk` 429 cm/cycle, run `PL_Base_L_Fist_RunAction`
  772 cm/cycle (pl_movaction), starts `PL_Idle_Idle2Run`/`Idle2Walk` (pl_movtrans), stops `PL_Base_L_Fist_RunToIdle`,
  `Bat_Base_R_Fist_WalkToIdle` (pl_transbase). Turn-start clips (`*_90L`, `_180R`, ...) exist but are not used yet;
  strafe/circle clips (`Jog_Weapon_CircleL/R`) are for lock-on movement.
- `som-game`: default mode is playable (WASD camera-relative, Ctrl = walk, mouse look, Esc frees the mouse),
  jumping was removed (the base game has none); `--viewer [--bank PATH]` is the old clip browser. `--hold KEY FROM TO` scripts input for unattended checks.

## Combat animations (first slice)
- Attack clips are named `Elf_Lt_L<level>_D<tier>_<F|L|R|B>_<startStance>_<endStance>_<n>`; L1 comes from
  `combatattackpool/attackpool_l1/pl_l1_d<tier>_<stance>.anix`, L2/L3 (stabs) from `cmbt_newl2l3lightchain`.
  `som-game/src/combat.rs` parses them into a `Catalog`; `Catalog::choose` picks by the target's side
  (F/L/R/B clips carry their own yaw: R = +90 game), distance (tier ~ authored reach 1.9 / ~3 / 4.1 m)
  and foot stance continuity (an attack ending `RR` is followed by one starting `RR`).
- Root travel is scaled (`Combat::warp`) so the clip ends at `STAND_OFF` from the target; the character is
  pre-turned so the clip's authored side faces the target. Far targets (> 4.8 m) first play an
  `Elf_GT_Run_D3_F_*` gap closer (`cmbt_getthere/elf_gettheres.anix`).
- Block: `Elf_Idle2Block` -> `Elf_Block_Idle` (loop) -> `Elf_Block2Idle`; hurt: `Elf_Recoil_{Light,Medium,Heavy}_*`
  in `elfmisc/elf_recoils.anix` (knock-back/-down variants need the ground game, not used). Controls: LMB/F
  attack, RMB/Q block, Space roll, H hurt (cycles weights).
- **Clip events**: each clip's `STRI` block is a cue track: from byte 0x0e, 8-byte records `{u16 time (10 ms),
  u16 text length incl. NUL, u16 offset, u16 0}`, count at +4; the cue texts sit in the group descriptor at the
  offset stored 4 bytes after the segment table (`anix::read_events`). Cues: `APPLYDAMAGE` (the blow),
  `BHITWIN`/`EHITWIN` (hit window), `BCOL`/`ECOL` (weapon collision), `BUFFTRANS` (next attack may start),
  `BCANWIN`, `AFACE`/`RFACE` (aim window), `RESETSTANCE`, plus audio hooks (`vx_PlayerMelee`, `cbt_wpn_*`).
  `som clips` with `SOM_EVENTS=1` prints them. The attack chain times its strike and next-attack point from these.
- Roll: `Elf_Dash_Chain_A` (`elf_block.anix`, cue `fol_ply_mvt_roll_dash`), Space; he turns to the stick direction
  first. No directional variants exist, so the roll always goes forward after the turn.
- Enemy health: dummies have 100 HP, damage 14/18/30 by chain level, bars float above them (UI projected with
  `Camera::world_to_viewport`), they topple at 0 HP and respawn after 4 s.
- Not wired: counter (sync actions), `pl_heavystabd*`, `elf_newheavy`, flurry, the DLC pools.

## Draw / stow and clip provenance (verified)

## Freeflow attack pools (decoded from `database/game/game.gamedb`)
The chain logic is not in the behavior graph (`player_elf.bvr` is a GADB of locomotion/climb/ride states: `BlendState`, `StateTransition`,
`Transition`...); it is data in `game.gamedb`. `som db <path> [type [item]]` dumps any GADB; `som attacks` lists Talion's 608 attacks.
- `Combat/Node:PC_PoolAttacks` -> `Combat/Pool` records `PC_PoolAttacks_HC_<tier>_D<n>`; `HC` = hit counter (combo count), tiers
  from `Combat/Predicates/HitCounter`: Lowest 0-3, Low 4-15, Medium 16-29, High 30+. D0..D3 = distance class. (Doubled/Tripled/FLM/
  DLC2 pools are alternates and are not used.) So the old "L1/L2/L3" were not levels: the tier is chosen by the hit counter.
- `Combat/Attack` -> `AnimationData` (clip name), `AttackData` (end stance, swing kind `Light_/Heavy_...`, translation scaling),
  `AttackPredicate` (allowed directions, **start** stance, distance, sword required). Stances: `LL/LR/RL/RR` = Left/Right foot, Left/Right
  shoulder forward (`Combat/AttackStance`). Next attack must start in the previous one's end stance.
- Kicks, elbows, thrusts and spin moves are ordinary pool members (my earlier exclusion of named specials was wrong). High tier uses the
  heavy `Elf_Hy_*` clips (`elf_newheavy`).
- Runtime (`combatdb.rs` + `combat.rs`): hit counter = landed hits, reset 4 s after the last hit or when hurt; candidates = tier by counter,
  allowed side, start stance = last end stance; ranked by reach fit, then pose gap, then repeat penalty.
- Distance classes are float ranges in cm to the target, stored in the string pool (`Combat/Predicates/Distance`, field kind 7 = offset to
  two `f32`): D0 0-75, D1 75-250, D2 250-450 (some pools 425), D3 450-700. Choose filters on them. `Arsenal/MeleeTranslationScaling`
  (`PC_PoolAttacks_Short/Long`) holds the root-travel stretch limits (floats, not applied yet; I clamp the warp to 0.3-1.7).
- Cancels: `Combat/CancelCondition` is for specials (getting there, tackles, AI grabs), not the light chain. The light chain is driven by clip cues:
  `BCANWIN` (roll/block may cancel from here; at 0 ms in the light clips), `BUFFTRANS` (next attack may start), `BTS`..`ETS` (target seek until
  impact), `STARTFACING`..`STOPFACING` (clip turns him toward the target), `BCOL/ECOL` (weapon collision), `BHITWIN/EHITWIN`, `APPLYDAMAGE`,
  `RESETSTANCE`. All are used now except BTS/ETS, BGA/EGA/BDW/AEE (meaning unclear) and BCOL/ECOL.
- Lock-on (soft, as in freeflow; the reference video shows no hard lock reticle, only the hit counter `xN` top left): the target is whoever the
  stick points at (else facing), kept until it dies or is >14 m away, swapped only when the stick points <30 degrees at another while the current
  one is >50 degrees off. Ring on the ground under it; attacks use it; the facing window steers toward it.
- Still undecoded: finishers (`PC_Flurry_*`, syncs: saved for the orc work), and what BGA/EGA/BDW/AEE mean.

## Orcs
- Banks: `animation/infantry/orc/orc_inf_{behavior,combat_attack,awareness}`, `shared/global/bip_recoils` (183 clips) and `bip_deathposes`,
  `shared/orc/orc_combat_block`, `orc_awareness`. Idle `Combat_Idle2`, `Combat_Walk`/`Combat_Run`, attacks `Bip_Cmbt_Attack_Front_1/3` (cues `STARTFACING`..
  `STOPFACING`, `APPLYDAMAGE`).
- Reactions are picked by the blow: `Orc_Recoil_<F|B>_<swing>_<100|200>cm` (F = attacker in front, swing = R2L/L2R/U2D/D2U/St from the attack's
  `Light_/Heavy_<Swing>` kind), heavy strikes and kills use `OrcInf_Recoil_Heavy_<swing>_<F|B>`; a kill ends on `Bip_DeathPose_01`.
- Brain (`orc.rs`): idle -> chase (walk/run by distance) -> attack (max 2 at a time) -> cooldown; blows reach the player through `IncomingBlows`
  (a roll's first 0.75 s evades, a block turns it aside). Placeholder: no counter prompt yet, orcs share one model, respawn after 6 s.
- Player: walks by default, Shift runs, C/Ctrl toggles crouch (stealth clip set from `pl_movaction`/`pl_movtrans`: `PL_Stlh_L_Idle`, `PL_Walk_Stlh`...).

## Counters, impact and test orcs
- There is no blocking in the game: removed. `RMB/Q` is the counter; an orc's wind-up (cue window `0.15 s .. APPLYDAMAGE-0.05`, within 3.5 m,
  facing the player) raises the `COUNTER [Q]` prompt. Answering it (Q, or a tap of attack) plays `PL_Bip_Sync_BasicCounter_<variant>` on Talion and
  `Bip_PL_Sync_BasicCounter_<variant>` on the orc (F variants when it is in front, B behind). The sync record `PC_Bip_Sync_Cntr_F` says
  `Attach` and carries no placement offsets I could use, so the orc is put 1.1 m in front of/behind him and its root motion is ignored;
  the kill lands at 1.0 s. Orc clips: some carry cues past their own end (`Bip_Cmbt_Attack_Front_*`: duration 1833 ms, cues to 4440 ms), so the
  orc blow uses the instant its weapon hand moves fastest (`Library::fastest_s`) when `APPLYDAMAGE` is past the end.
- Player blows now land on weapon contact (the `R_Weapon` hand within 0.75 m of a target inside the clip's `BCOL..ECOL` window), at the latest on
  `APPLYDAMAGE`; each hit freezes time briefly (`HitStop`: 0.07 s light, 0.12 s heavy, via `Time<Virtual>` speed) and throws sparks.
- Test setup: orcs are `Cha_Orc_Defender` dummies that only watch; `T` provokes the nearest one to swing, `SOM_HOSTILE=1` makes them fight.
- Tools: `som db`, `som dbfind <gadb> <text>`, `som attacks`, `SOM_ROOT0`, `SOM_SWING`, `SOM_SEGS`, `SOM_SKEL` on `som clips`.

## Finishers, stealth kills, sync placement
- **Stun -> flurry -> ender** (from `Combat/Node` `PC_Flurry_Standing_1..6` / `_END`; the ender's predicate needs the target in `Combat/States/List:Quick_Stunned`,
  a PC_Sword, a biped, front/back direction and distance `PC_Flurry_End`): a blow leaves the orc stunned 1.8 s; hitting a stunned orc within 2.8 m plays
  `Elf_Cmbt_FlurryStand_*` flurry hits, and after 3 flurry hits (or the orc at <=35 HP) the ender `PL_Bip_Sync_Flurry_Ender_Kill_F` pairs with the orc's
  `Bip_PL_Sync_Flurry_Ender_Kill_F` (in `bip_recoils`). The stun length and the "3 hits / 35 HP" thresholds are mine, not decoded. `SOM_FLURRY=n` overrides.
- **Stealth kill**: crouched, an orc that has not noticed him (dummies only watch the player within 5.5 m and not while he crouches), LMB within 2.6 m:
  `PL_Bip_Sync_StealthKill_{B_SkullStab,B_StabNeckBreak,B_StabOpen,F_TakeDownStab}` with `Bip_PL_Sync_StealthKill_*` (from behind: orc a metre ahead, same heading).
- **Sync placement** stays a fixed 0.9-1.1 m placement. `som syncfit` (player bank, prefix, victim bank, prefix, suffixes...) fits the victim's start offset/yaw by
  matching the blade point to the victim's torso around the blow; unconstrained fits are underdetermined (offsets of 1-4 m, residual 7-18 cm) and the
  constrained ones (110 cm in front, facing) leave 75-225 cm residual, so I do not use them. The fixed placement looks right for the stealth kill and ender.
- **Hit timing**: attacks used to reach the target only after their blow (warp scaled the whole clip), so the blow fired at the cue with the blade still ~1.4 m
  away. The warp now covers the gap by the blow (`Attack::strike_reach`, the root's travel at `APPLYDAMAGE`); contact is tested along the blade
  (grip to 102 cm, +Y of `R_Weapon`) against the target, not before `APPLYDAMAGE - 0.25 s`. Recoil clips have no dead lead-in (measured 0 s).
- `--burst FROM STEP COUNT` with `--shot` saves a numbered frame sequence.

## Move selection (game nodes), Ghidra findings, dagger
- **Field names**: the database stores field names as hashes, but the exe keeps them as reflection strings: scanning the exe's ASCII strings with the name hash
  resolves 6308 of the 6317 field hashes (`som db` now prints real names: `AttackExplicitValue`, `StateTargetList`, `TargetParryType`, `HitCounterRange`...).
- **Engine predicate evaluators** (Ghidra, debug strings survive): `FUN_1404b9640` = `EvaluteTarget` (checks in order: distance, vertical offset, **ParryWindowOpen**
  [target's current attack, flag at +0x562], **CounterTarget** [CanCounter], line of sight, on screen, health range, character type, items, direction, combat states...),
  `FUN_1404c2c70` = `EvaluteSelf` (items, behavior states, health, buffs, quests...). `FUN_140a6e000` and siblings are the field-descriptor constructors (name hash,
  type) - the hash is `h*0x397 + rank`, as used everywhere.
- **Nodes** (`combatnodes.rs`, `som moves [prefix]`): `Combat/Node` = input (`Light`, `Counter`, `Heavy`, `Dash`...), priority (the node's `type`; **lower number first**:
  `PC_Counter` 4 before `PC_Counter_Fail` 13), pools -> attacks. Attack predicate fields used: `item` (`PC_Sword`/`PC_SwordOrDagger`), `StateTargetList`
  (`Quick_Stunned`, `NormalOrStunned_NotQuickStnd`, `KnockedDown`), `TargetParryType` (`PC_ParrySuccess`), `AllowedDirection`/`DirectionAllowedToAttacker`,
  `distance`, `AttackStance`, `AttackExplicitValue` (highest wins; -1 lowest), `HasInventoryItem`/`DoesNotHaveInventoryItem` (`CSP_HeadExp` = gore setting on),
  `TargetHasInventoryItem`/`TargetDoesNotHaveInvItem` (resistant/immune/named), `TargetHealthRange`, pool `HitCounterRange`. `Select::select` in `combat.rs` walks them.
- **Counter**: needs the enemy's parry window (cues `BPW`..`EPW`); pressing Counter with none picks `PC_Counter_Fail` -> `Elf_ParryFail`. Pairs come from `Combat/Sync`
  (`SourceAttack` -> `TargetAttack`). **Finisher**: `PC_Flurry_Standing_END*` (value 100-210) outranks the flurry hits (-1) whenever the target is `Quick_Stunned` and within
  300 cm, so a stunned orc is finished at once (the flurry only runs when no ender qualifies). What puts an orc in `Quick_Stunned` is not decoded: here a blow that leaves it <=55 HP does.
  Heavy input (`PC_PoolAttacks_Hvy`, ground strikes on `KnockedDown`) and the named/special counters are not wired.
- **Cue timelines**: some clips (orc attacks, sync pairs) carry cues stretched past their key data (last cue ~2x the clip); `Library::event_s` scales them by `duration/last cue`
  when the last cue is >1.15x the duration, which puts orc blows where the arm swings.

## Conventions
- Units are centimetres, Y up, left-handed with Z forward. `som-game` mirrors Z for Bevy:
  positions `(x, y, -z) * 0.01`, quaternions `(-x, -y, z, w)`.
- Quaternions are `(x, y, z, w)`; world = parent * local.
- Animation time is in milliseconds; keys sit ~33 ms apart.

## Reverse engineering workflow
- Ghidra project for `x64/ShadowOfMordor.exe` lives outside the repo; `tools/ghidra/query.ps1`
  runs `Query.java` against it (decompile with callees, symbol/string search, xrefs, constant
  search). Decompiler output is written outside the repo and must not be committed.
- Loaders are found by their file magic (`FindMagic.java`); the animation codecs by their
  `EvaluateBlock<TAG> decoded NAN` strings, including plain-C `_Reference` versions.

## Open questions
- Patch bundle layout in DLC2/NF_Patch (needed for DLC skins).
- ANIX: the `STRI` codec and the third channel (hash 0x210c8f5a) on the player; clip events
  (hit frames, cancel windows) have not been looked for yet.
- Cloth (`.hkt`, Havok) is not simulated: cloak, hair and pouches follow their bind pose.

## Stun trigger (decoded), hit timing, orc kinds
- **What puts an orc in `Quick_Stunned`** (`som applied PC_`, `som stats stun`): every attack `Combat/Category` has `AppliedState` records (`BaseChance`, `State`).
  Freeflow pool hits (`PC_PoolAttacks_HC_*`) and counters apply plain `Stunned` (chance 1.0, 0.75 for HC_Low); **`Quick_Stunned` is applied only by the Stun
  attacks (`PC_Stun_HC_*`, input `Stun_Release_NoCancel`, clips `PL_St_L1_D1|D2_*` in `pl_stunattackpool.anix`) and by the flurry's own hits
  (`PC_Flurry_Standing_Chain_*`, 100%, `QSTN_100PCT_ShortDur`)**. So: Stun, then Light -> flurry/ender. `Combat/Stats/StatEntries` (key: attacker category,
  direction, defender state, items -> `StateMods` with additive/multiplicative factors) only scale those chances (e.g. Berserkers x0.25 stun from the front,
  shielded Defenders x0). State durations are expressions that are not decoded; I use 5 s after a Stun and 3 s per flurry hit. Here: key `E`; close Stun moves need
  `CSP_Wraith_Punch_2` (not given), the basic `CP_BasicWraith` ones are.
- **Hit delay (measured)**: the blade passes the orc ~0.84 s into the clip, the `APPLYDAMAGE` cue is at 1.23 s. The blow now fires when the blade has passed its closest
  point to a target in front (<1 m) inside BCOL..ECOL; the cue is only the fallback.

## Native stun / finisher / hit logic (second pass; user's native video)
- **Controls now**: LMB = freeflow attack only, **F = finisher** on a stunned orc (prompt `FINISH [F]` floats over it, like the video's click prompt), Q/RMB counter,
  Space roll, C crouch. There is no Stun button (my earlier `E` Stun move was wrong): `Quick_Stunned` Stun attacks are a separate player move, orcs in the
  video are stunned by plain combat.
- **Stun from combat** (`Combat/Category.AppliedState`): each freeflow pool hit applies `Stunned` with `BaseChance` 1.0 (0.75 for the HC_Low tier); here a stunned orc
  stays stunned 3 s (duration expression not decoded), renewed by each hit, and that is what enables the finisher (the flurry/ender attack predicates name the
  list `Quick_Stunned`; how the engine maps `Stunned` onto it was not found, so this is the documented approximation).
- **Finisher selection**: `Select::select("Light")` restricted to a stunned target, now ignoring the ledge kick (`PL_LedgeKick`, value 1000, in `PC_PoolAttacks`) and
  the KD pools; the ender (`PC_StandingFlurry_END_Kill_F/B`) outranks the flurry hits, so F plays `PL_Bip_Sync_Flurry_Ender_Kill_*` straight away.
- **Ghidra (`FUN_140c19460`, the `APPLYDAMAGE` cue handler)**: native damage is *not* a blade-contact test. At the `APPLYDAMAGE` cue the engine checks the distance to the
  target against `ApplyDamageRange` (+ an effective extension when the attacker is moving) and the angle against `ApplyDamageMaxHalfAngle`, then applies the hit
  (`FUN_140c15570`) in the same call. Cue handlers also exist for `BSLOWMO`/`ESLOWMO` (hit-counter slow-mo, `Combat/SlowMo`, `SlowMo:HitCounterSlowMo`) and
  `BTS`/`ETS` are translation-scaling (root-motion warp) windows, not playback-rate changes. Here the blow fires at the blade's closest approach (visible contact)
  and the cue is the fallback, so the hit reaction starts on the frame you see the blade land.
- **One dummy**: a single orc grunt (`Cha_Orc_Infantry`), 5 m ahead.

## Stun length decoded, knock-down flow, all counter families (third pass)
- **Durations and chances are data** (`som durations`, `som moves <node>` shows `applied`): `Combat/Category.AppliedState` records hold `BaseChance`, `State` and a
  `Duration` that is a **pointer into the database float pool** (kind 7, not a string; `Entry::raw_field` + `GameDb::pool_f32`). Each attack's categories
  (`AttackDef.applied`, with the category's `PredicateHitCounterRange`) give: lowest freeflow tier (hits 0-3) applies nothing; low tier `Stunned` 75% for 2.0 s;
  medium `Stunned` 3.0 s; high tier (30+) `Stunned` + `KnockDown` 4.0 s; Stun attack `Quick_Stunned` 1.0 s; flurry chain `Quick_Stunned` 3.0 s; counters by tier:
  stun 0.5/1.0/1.5/2.0 s, knock-down 1.2/1.5/2.0/2.5 s. `DurationOverrides` (+1.5 s etc.) need the upgrade item `BD_Melee_S_KD_Dur` and are not applied.
- **Knock-down flow**: a plain-`Stunned` orc is a `StunnedOnly` target, so LMB selects `PC_PoolAttacks_KD` (77 attacks `PL_KD_L1_D*`: sweeps, leg sweeps, flying
  knee, shoulder ram, explicit value 20, bank `wulfmadeanims/pl_knockdownlegsweep.anix`); `KnockDown_100PCT` 3 s puts the orc down (`Orc_Recoil_<F|B>_KD_*`, leg sweeps
  `Orc_Recoil_LegSweep_*`, then `Orc_Recoil_KDLoop` and `Orc_Recoil_KDGetup`). Stunned orcs hold `Orc_Recoil_Stunned` and end with `Orc_Recoil_StunRecover`.
  KD clips use the short damage cue `AD` (the engine's cue handler takes `APPLYDAMAGE`/`AD`/`BAD`...), `Library::event_s("APPLYDAMAGE")` falls back to it.
- **Ground strikes** (`PC_PoolAttacks_Hvy/_Heavy`, target `KnockedDown`, clips `Elf_Hy_L1_D*_GrndStrk*`): the clips are in **no** `animation/**.anix` bank of this install
  (`som findclip GrndStrk animation` finds nothing), so a downed orc takes the normal chain instead.
- **Dash attack**: sprinting (Shift, > 3 m/s) + LMB selects `Dash_Attack` (`PC_DashAttack_Short` 0-2 m, `PC_DashAttack` 2-7 m, clip `Elf_DashAttack`).
- **Counters**: all four families of the game's data: `PC_Counter_Basic` (hit counter <= 8, kills), `PC_Counter_Stun`, `_Knockdown`, `_Knockback` (survive; stun / knock-down
  state from the data), `_Kill` (kills; instakill variants need `CSP_Chord_InstaK`). Which family a given prompt offers is not decoded: they take turns here.
- New tools: `som findclip <name> [path]`, `som durations`, `som stats`, `som applied`, `som skel`.

## Freeflow pacing, Execution, slow motion, combo counter, the grunt (fourth pass; native videos 10-04-43 / 10-08-16)
- **Native pacing** (combo counter of the video read at 10 fps): consecutive hits land 0.3-0.9 s apart. Ours were 1.6-2 s. Now 0.38-0.94 s.
- **Engine cue flags** (Ghidra `FUN_1408c7b70`, the combat model's cue handler, debug strings "Begin Cancel Window", "Begin Hit Window", "Transition", "Enable Auto
  Links", "Start Parry Window"): `BCANWIN/ECANWIN` cancel window (+0x31), `BHITWIN/EHITWIN` hit window (+0x33), `BUFFTRANS` buffered transition (+0x32/+0x36), `EAL`
  auto links (+0x37), `BPW/EPW` parry window (+0x39), `BDW/EDW` (+0x34), `AEE` early exit. `FUN_140c28b40`: `BCOL`/`ECOL` create/remove the weapon collider.
  `Combat/SlowMo:Player` times its slow motion relative to **BCOL** (-200..-100 ms), i.e. the impact is at the collider window, not at the later `APPLYDAMAGE`.
- **What the game does here now**: the blow lands when the collider window (`BCOL`..`ECOL`) is open and the target is within reach in front (`APPLYDAMAGE`/`AD` only as
  fallback); during the wind-up Talion is carried to striking distance of the target wherever it moved (translation scaling, arriving at `BCOL`); the pool attacks' cancel
  window is open from frame 0, so a buffered attack cuts in 0.12 s after a landed blow (`BUFFTRANS` only when nothing was hit). `STAND_OFF` 1.2 m.
- **Cue timelines**: `PlaybackRate` is 1.0 for all attacks (float pool); clips whose cues run past their data (KD moves 1.7 s data / 2.57 s cues, orc attacks 1.83 / 4.44 s)
  have one segment of exactly the data length, so the cue times are on a stretched authoring timeline: `Library::cue_scale` stays.
- **Kind-7 fields are two floats in the float pool**: for `SlowMo` time scales a LithTech fraction (numerator, denominator): `HitCounterSlowMo` 1/7, `KOSlomo` 1/8,
  `KDSlowMo` 1/20 (player x4), `Elf_TokenKill_Slomo` 1/5, ramps 0.07 s in / 0.66 s out; for `AppliedState.Duration` a **range** (stun low tier 2-3 s, medium 3-4, high 4-5,
  lowest 0.5-1; KD 3-6). `Applied.duration`/`duration_max`, `som slowmo`, `som durations`.
- **Slow motion** (`combat::SlowMo`, through `Time<Virtual>`): on the blow that will kill the target (`KOSlomo`) and on the hit that charges the streak (`HitCounterSlowMo`),
  from 200 ms to 100 ms of clip time before `BCOL`, then the ramp out. Which hits the game's `HCL`/`HCC`/`KB` entries select is my reading of the names.
- **Execution** (upgrade screen in the video: "When your Hit Streak is charged ... execute your target. Hit Streak Charged: every x8 hits ... your sword will glow"): node
  `PC_InstaKill_KBM` (input `TokenKill_KBM`, 90 attacks, item `CSP_Chord_InstaK`): standing kills `PL_Bip_Sync_InstaKill_F_*`/`_B_*` and ground kills on a knocked-down orc
  `PL_Bip_Sync_Instakill_Gr_*`/`OnGr_*` (banks `pl_syncaction_chordkill_stand/_onground` + `orc_syncaction_chordkill_*`). F spends the charge (gained at every 8th
  consecutive hit, lost with the streak). The old "F on a stunned orc" is gone; `EXECUTE [F]` shows while charged.
- **Combo counter**: `xN` top left with a streak bar, pops on each hit, red when charged; the sword glows (emissive) while charged.
- **Grunt orc** (video 1): composed from `Model` records `Smp_Orc_Buff_Torso01` + `Smp_Orc_Buff_Head01` + `Smp_Orc_Buff_Legs01` (each piece carries the same 108-bone
  skeleton; one set of joints per piece driven by the same pose) with `Wep_Orc_Mace01` in `R_Weapon`. `SOM_ORC_HP=n` makes a tougher training orc; respawn 2.5 s.

## Attack audit and the light chain (fifth pass)
- **`som audit [out.tsv]`** checks every attack of the player's combat nodes against what the game loads (`som_formats::banks`, shared with the game): clip present / in
  which unloaded bank / in no bank, cues (`BCOL`, `APPLYDAMAGE`/`AD`, `ECOL`, `BUFFTRANS`), cue scale, travel at the blow, the clip's own turn, and the data's stance/side
  against the tokens in the clip name. Results: `PC_PoolAttacks` 420 of 422 clips load (2 are in no bank on disk), all with collider and damage cues; KD 77/77;
  dash attack 3/3; counters Basic/Stun/Knockdown/Knockback/Kill all load; `PC_InstaKill_KBM` loads every plain-orc variant. The rest of the misses are other creatures
  (goblin, warg, spider, Black Hand, defender shields, DLC).
- **What the audit changed**: (1) back and side attacks often do not turn at all (back kicks, reverse stabs: "turns 0 deg for side B") - only front-authored, non-spinning
  attacks are steered at the target now, and the blow test no longer asks a back kick to face its victim; (2) ~40 records disagree with their clip name on stance or side -
  the data's stances are used for chaining, the clip's own side for lining up; (3) two synchronised moves have no orc half in this install
  (`Elf_OrcInf_Sync_Cntr_KD_B3`, `PL_Bip_Sync_Instakill_Gr_R_HeadChop1`) - moves without a victim clip are pruned at start (`Player::prune_moves`); (4) the vault was in
  banks we did not load (`elf_sync.anix`, `orc_syncaction.anix`).
- **Light chain = the game's rule**: all 422 pool attacks carry `AttackExplicitValue` 20, so nothing ranks them; the candidates are those whose predicates hold (hit-counter
  tier, side, distance class, start stance == current stance) and the pool's `VariationType` (`Pool_Basic_Attacks`, a bare key) keeps the pick from repeating. `Catalog::choose`
  does exactly that now (random among valid, skipping the last few); my reach-fit and pose-gap ranking is gone. A clip chained into itself restarts (`Animator::restart`).
- **Vault**: Space with an orc within 2 m in the way selects `PC_DashOver` (`PL_Bip_Sync_JumpOver` / `_B` with `Bip_PL_Sync_JumpOver*`), a harmless sync; otherwise the roll.

## Twenty-sixth pass: steady ground states take their clip and rate from the graph

- `GaitGraph::steady(state, GroundCtx, lib)` walks a ground state's clip tree (`Blend` along `Dampen`/`If`/`Game_NodeLifeTime`/`RawMoveMag` values, nearest point) and evaluates its `Rate`. `GroundCtx` supplies the natives: `Stealth`/`CanHide` = crouched,
  `InCombat` = sword drawn (my reading; the exe's rule is not decoded), `Game_NodeLifeTime` = seconds in the gait. `Game_GroundPitch` = 0 (flat floor), other values are the graph's defaults.
- Walking (no Shift) plays the graph's `Walking` (`PL_Walk_Standard`, rate `Speed_Walk * WalkAndTalkScale`), running (Shift) plays `Sprint` (`PL_Hustle85` until 1.5 s, then `PL_Sprint85`, switching with a cross-fade at the same phase).
  Both replace my guessed `PL_Cmbt_Walk` / `PL_Base_L_Fist_RunAction` (run speed 5.8 -> 6.4 m/s from the clips' own root travel). Stops are `Walking_To_Idle` and `RunToIdle_L/R` (the side is read from the cycle phase, approximate).
  `SOM_GAIT_DEBUG=1` logs the picked clip and rate.
- **Still mine:** the transitions between idle / start / moving / turning / stopping (`Loco`), the `Idle_To_*` start clips and turns, and the interpolation between neighbouring blend points (nearest point only; `Run` blends Sprint85/Jog/WalkFast
  by node time and goes to `Walking` after 1.5 s in the graph - not modelled because my gait never enters `Run`).

## Twenty-seventh pass: combat flow

- A hit-stop no longer freezes the simulation (relative speed 0.04 -> 0.2 for its short duration); after a blow has landed, a held stick carries him from the attack into the gait at once (`FOLLOW_THROUGH * 2`) instead of at the clip's `BUFFTRANS` cue.
- Run / walk -> attack -> run / walk checked by script (`--hold w/shift`, `--hold l`): the gait restarts at `Game_NodeLifeTime` 0 after each attack. Whiffed swings still chain at `BUFFTRANS` (about 1.35 s into the clip); hits chain 0.5-0.8 s apart.

## Twenty-eighth pass: no slow motion, stick figures

- Hit-counter / kill slow motion removed (the hit-stop stays, short and never a full freeze).


## Open-source build

- Slow motion (described in the fourth pass) is not part of this build; the hit-stop is short and never a full freeze.
- Gait clips and rates come from the player behaviour graph (`gait.rs`); the camera uses the graph's `Camera_OnGround` profile.

- Draw / sheathe animations are not part of this build: the sword is always in his hand.
- Counters: only `PC_Counter_Kill` finishes the orc; the basic counter and the others leave it stunned / knocked down / knocked back.
- The player and the orc are the same generic 26-bone humanoid (`Skeleton::humanoid`, names as the animation banks use them); the animations drive it through their own per-node transforms, so no game skeleton is read.
