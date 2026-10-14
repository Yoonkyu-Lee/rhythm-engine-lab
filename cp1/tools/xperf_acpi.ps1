# CP1-3: 87 ms ACPI DPC가 어떤 ACPI 명령인지 가리기 위한 기록 (관리자 권한으로 실행).
#   커널: DPC/ISR + 1 ms CPU 샘플링 스택 (DPC 안에서 ACPI.sys가 어느 함수에 있었는지)
#   사용자 공급자: ACPI·온도·배터리·전력계 조회 이벤트 (DPC 시각과 맞춰 본다)
#   powershell -ExecutionPolicy Bypass -File xperf_acpi.ps1 -Dir <출력 폴더> -Seconds 330
param([string]$Dir, [int]$Seconds = 330)
$xperf = "${env:ProgramFiles(x86)}\Windows Kits\10\Windows Performance Toolkit\xperf.exe"
New-Item -ItemType Directory -Force $Dir | Out-Null
Set-Location $Dir
$prov = @(
  "C514638F-7723-485B-BCFC-96565D735D4A:0xFFFFFFFFFFFFFFFF:5",  # Microsoft-Windows-Kernel-Acpi
  "DAB01D4D-2D48-477D-B1C3-DAAD0CE6F06B:0xFFFFFFFF:5",          # ACPI Driver Trace Provider (WPP)
  "5885720D-F086-4614-A17C-4770C2461AF2:0xFFFFFFFFFFFFFFFF:5",  # DptfAcpiEtwProvider (Intel 온도·전력 관리)
  "59819D0A-ADAF-46B2-8D7C-990BC39C7C15:0xFFFFFFFFFFFFFFFF:5",  # Microsoft-Windows-Battery
  "E8A7C168-81EE-465C-8E8E-D39A2AC1CA41:0xFFFFFFFFFFFFFFFF:5",  # Microsoft-Windows-Thermal-Polling
  "306C4E0B-E148-543D-315B-C618EB93157C:0xFFFFFFFFFFFFFFFF:5"   # Microsoft-Windows-Power-Meter-Polling
) -join "+"
& $xperf -on PROC_THREAD+LOADER+DPC+INTERRUPT+PROFILE -stackwalk Profile -BufferSize 1024 -MinBuffers 256 -MaxBuffers 1024 -f "$Dir\kernel.etl" -start acpi -on $prov -BufferSize 1024 -MinBuffers 64 -MaxBuffers 512 -f "$Dir\user.etl" *> "$Dir\on.log"
"started $(Get-Date -Format o)" | Out-File "$Dir\started.flag"
Start-Sleep -Seconds $Seconds
& $xperf -stop acpi -stop -d "$Dir\trace.etl" *> "$Dir\off.log"
"done $(Get-Date -Format o)" | Out-File "$Dir\done.flag"
