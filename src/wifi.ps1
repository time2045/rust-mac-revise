$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$guid = [guid]'__GUID__'
$profiles = @()
$unreadable = 0
$dir = Join-Path $env:ProgramData ("Microsoft\Wlansvc\Profiles\Interfaces\" + $guid.ToString('B'))
if (Test-Path -LiteralPath $dir) {
    Get-ChildItem -LiteralPath $dir -Filter *.xml -File -ErrorAction SilentlyContinue | ForEach-Object {
        try {
            $text = [System.IO.File]::ReadAllText($_.FullName, [System.Text.Encoding]::UTF8)
            [xml]$xml = $text
            $nameNode = $xml.GetElementsByTagName('name') | Select-Object -First 1
            $randomNode = $xml.GetElementsByTagName('enableRandomization') | Select-Object -First 1
            if ($nameNode) {
                $random = $false
                if ($randomNode) { $random = ([string]$randomNode.InnerText).Trim().ToLowerInvariant() -eq 'true' }
                if ($random) { $profiles += [pscustomobject]@{ ssid = [string]$nameNode.InnerText; randomization = $true } }
            }
        } catch {
            $unreadable++
        }
    }
}
$current = $null
try {
    $adapter = Get-NetAdapter -IncludeHidden -ErrorAction Stop | Where-Object { [guid]$_.InterfaceGuid -eq $guid } | Select-Object -First 1
    if ($adapter) {
        $profile = Get-NetConnectionProfile -InterfaceAlias $adapter.Name -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($profile) { $current = [string]$profile.Name }
    }
} catch {}
[pscustomobject]@{ profiles = $profiles; current_ssid = $current; unreadable = $unreadable } | ConvertTo-Json -Depth 3 -Compress
