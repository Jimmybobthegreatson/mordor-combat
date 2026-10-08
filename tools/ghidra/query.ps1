# Run tools/ghidra/Query.java against the analysed ShadowOfMordor.exe project.
#   .\query.ps1 <outName> dec <depth> <addr> [<addr> ...]
#   .\query.ps1 <outName> sym <regex>
#   .\query.ps1 <outName> xref <addr> [<addr> ...]
#   .\query.ps1 <outName> str <regex>
# Output lands in <GHIDRA_PROJECTS>\decomp\<outName>.c, outside the repo (retail code is never committed).
param(
    [Parameter(Mandatory)][string]$OutName,
    [Parameter(Mandatory, ValueFromRemainingArguments)][string[]]$QueryArgs
)
$ghidraRoot = if ($env:GHIDRA_DIR) { $env:GHIDRA_DIR } else { 'C:\Users\Shadow\tools\ghidra_12.1.4_PUBLIC' }
$projects = if ($env:GHIDRA_PROJECTS) { $env:GHIDRA_PROJECTS } else { 'C:\Users\Shadow\tools\ghidra-projects' }
$env:JAVA_HOME = if ($env:JAVA_HOME) { $env:JAVA_HOME } else { 'C:\Program Files\Eclipse Adoptium\jdk-21.0.12.101-hotspot' }
$env:PATH = "$env:JAVA_HOME\bin;$env:PATH"

$outDir = Join-Path $projects 'decomp'
New-Item -ItemType Directory -Force $outDir | Out-Null
$outFile = Join-Path $outDir "$OutName.c"

& "$ghidraRoot\support\analyzeHeadless.bat" $projects som -process ShadowOfMordor.exe -noanalysis `
    -scriptPath $PSScriptRoot -postScript Query.java $outFile @QueryArgs *> (Join-Path $outDir "$OutName.log")
Write-Output $outFile
