#!/usr/bin/env bash
# CP2 2-5 비교 실험: "고장 시나리오 × 프로필". 고장 하나를 곡 10초에 넣고 22초에 끝낸다 (시나리오끼리 영향이 섞이지 않게).
# 쓰는 법 (cp2/에서): bash tools/run_matrix.sh [곡.rtc]  → runs/m-<프로필>-<시나리오>-*
set -u
SONG=${1:-content/synth/synth.rtc}
declare -A SC=([clean]="" [audio150]="audio@10:150" [render300]="render@10:300" [judge500]="judge@10:500" [input300]="input@10:300" [audio50]="audio@10:50")
[ "${EXTRA_ONLY:-0}" = 1 ] || for p in ours iidx djmax bms; do
  for s in clean audio150 render300 judge500 input300 audio50; do
    f=${SC[$s]}
    ./target/release/cp2game.exe run "$SONG" --bot --no-capture --profile $p --stop-at 22 --late-drop-ms 0 --resync-judge count --tag m-$p-$s ${f:+--faults $f} 2>&1 | tail -1 | sed "s/^/$p $s: /"
  done
done
# 추가 실험 (노트가 0.5초 간격이라 고장 위상을 노트에 맞춘다)
# 1) 재동기화 중 판정: 오디오 정지를 노트 0.3초 전에 → 시계 점프 감지 뒤 0.15초에 노트. 사람 흉내 봇(300 ms 뒤에 알아챔) vs 즉시 따르는 봇
AF=audio@10.2:150,audio@20.2:150,audio@30.2:150
for ad in 300 0; do
  ./target/release/cp2game.exe run "$SONG" --bot --no-capture --profile ours --stop-at 40 --bot-adapt-ms $ad --resync-judge count --tag x-adapt$ad-ours --faults $AF 2>&1 | tail -1 | sed "s/^/adapt $ad: /"
done
./target/release/cp2game.exe run "$SONG" --bot --no-capture --profile ours --stop-at 40 --bot-adapt-ms 300 --tag x-adapt300void-ours --faults $AF 2>&1 | tail -1 | sed "s/^/adapt 300 void: /"
# 2) 늦은 키음: 정지 중에 누른 노트의 키음이 300/250/400 ms 늦게 요청된다. 밀기 vs 100 ms 넘으면 버리기
LF=judge@9.8:500,input@19.95:300,judge@29.9:500
for dr in 100 0; do
  ./target/release/cp2game.exe run "$SONG" --bot --no-capture --profile ours --stop-at 40 --late-drop-ms $dr --tag x-drop$dr-ours --faults $LF 2>&1 | tail -1 | sed "s/^/drop $dr: /"
done
