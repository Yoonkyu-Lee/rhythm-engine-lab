# CP1-3: 커널 DPC/ISR 기록 (관리자 권한으로 실행). 시작 → 정해진 시간 대기 → 종료·합치기를 한 번에 해서 UAC 창이 한 번만 뜨게 한다.
#   powershell -ExecutionPolicy Bypass -File xperf_record.ps1 -Dir <출력 폴더> -Seconds 630
param([string]$Dir, [int]$Seconds = 630)
$xperf = "${env:ProgramFiles(x86)}\Windows Kits\10\Windows Performance Toolkit\xperf.exe"
New-Item -ItemType Directory -Force $Dir | Out-Null
Set-Location $Dir
# PROC_THREAD + LOADER: 주소를 드라이버 파일로 바꾸는 데 필요. DPC + INTERRUPT: 드라이버가 CPU를 붙잡은 시간.
& $xperf -on PROC_THREAD+LOADER+DPC+INTERRUPT -BufferSize 1024 -MinBuffers 128 -MaxBuffers 1024 -f "$Dir\kernel.etl" *> "$Dir\on.log"
"started $(Get-Date -Format o)" | Out-File "$Dir\started.flag"
Start-Sleep -Seconds $Seconds
& $xperf -d "$Dir\trace.etl" *> "$Dir\off.log"
"done $(Get-Date -Format o)" | Out-File "$Dir\done.flag"
