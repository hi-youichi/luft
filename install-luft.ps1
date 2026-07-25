# Luft 一键安装脚本
# 检测支持的 agent，安装技能和 MCP

param(
    [switch]$Verbose,
    [switch]$SkipCodexCheck
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Write-Step {
    param([string]$Message)
    Write-Host "🔧 $Message" -ForegroundColor Cyan
}

function Write-Success {
    param([string]$Message)
    Write-Host "✅ $Message" -ForegroundColor Green
}

function Write-Warning {
    param([string]$Message)
    Write-Host "⚠️  $Message" -ForegroundColor Yellow
}

function Write-Error {
    param([string]$Message)
    Write-Host "❌ $Message" -ForegroundColor Red
}

# 检测可用的 ACP 后端
function Test-BackendAvailable {
    param([string]$BackendId)
    
    switch ($BackendId) {
        "mock" {
            return $true  # Mock 总是可用
        }
        "codex" {
            return Test-NpxPackageAvailable -PackageName "@agentclientprotocol/codex-acp"
        }
        "opencode" {
            return Test-OpencodeAvailable
        }
        "claude" {
            return Test-ClaudeCodeAvailable
        }
        default {
            return $false
        }
    }
}

function Test-NpxPackageAvailable {
    param([string]$PackageName)
    
    try {
        $result = npx -y $PackageName --version 2>&1
        return $LASTEXITCODE -eq 0
    } catch {
        return $false
    }
}

function Test-OpencodeAvailable {
    # 检查是否有 OpenCode 的迹象
    try {
        # 检查常见的 OpenCode 安装路径或命令
        $paths = @(
            "${env:USERPROFILE}\.opencode",
            "${env:APPDATA}\opencode",
            "${env:LOCALAPPDATA}\opencode"
        )
        
        foreach ($path in $paths) {
            if (Test-Path $path) {
                return $true
            }
        }
        
        # 检查是否有 opencode 命令
        if (Get-Command opencode -ErrorAction SilentlyContinue) {
            return $true
        }
        
        return $false
    } catch {
        return $false
    }
}

function Test-ClaudeCodeAvailable {
    # 检查 Claude Code 是否可用
    try {
        # 检查常见的 Claude Code 路径
        $paths = @(
            "${env:USERPROFILE}\.claude",
            "${env:APPDATA}\claude",
            "${env:LOCALAPPDATA}\claude"
        )
        
        foreach ($path in $paths) {
            if (Test-Path $path) {
                return $true
            }
        }
        
        return $false
    } catch {
        return $false
    }
}

# 安装 Codex ACP
function Install-CodexACP {
    Write-Step "检查 Codex ACP 安装状态..."
    
    if (Test-NpxPackageAvailable -PackageName "@agentclientprotocol/codex-acp") {
        Write-Success "Codex ACP 已可用 (通过 npx)"
        
        # 检查是否全局安装
        $globalInstall = npm list -g @agentclientprotocol/codex-acp 2>&1
        if ($LASTEXITCODE -eq 0) {
            Write-Success "Codex ACP 已全局安装"
        } else {
            Write-Warning "Codex ACP 仅通过 npx 可用，建议全局安装"
            Write-Host "运行: npm install -g @agentclientprotocol/codex-acp" -ForegroundColor Gray
        }
        return $true
    } else {
        Write-Warning "Codex ACP 不可用"
        return $false
    }
}

# 创建技能目录结构
function Initialize-SkillDirectories {
    param([hashtable]$AvailableBackends)
    
    Write-Step "初始化技能目录结构..."
    
    $skillDirs = @()
    
    if ($AvailableBackends["codex"] -or $AvailableBackends["opencode"]) {
        $codexOpencodeDir = Join-Path $env:USERPROFILE ".agents\skills\workflow"
        if (-not (Test-Path $codexOpencodeDir)) {
            New-Item -ItemType Directory -Path $codexOpencodeDir -Force | Out-Null
            Write-Success "创建目录: $codexOpencodeDir"
        }
        $skillDirs += $codexOpencodeDir
    }
    
    if ($AvailableBackends["claude"]) {
        $claudeDir = Join-Path $env:USERPROFILE ".claude\skills\workflow"
        if (-not (Test-Path $claudeDir)) {
            New-Item -ItemType Directory -Path $claudeDir -Force | Out-Null
            Write-Success "创建目录: $claudeDir"
        }
        $skillDirs += $claudeDir
    }
    
    return $skillDirs
}

# 复制技能文件
function Copy-SkillFiles {
    param([string[]]$TargetDirs)
    
    Write-Step "复制技能文件..."
    
    $sourceSkillDir = Join-Path $PSScriptRoot ".loom\skills\auto"
    
    if (-not (Test-Path $sourceSkillDir)) {
        Write-Warning "源技能目录不存在: $sourceSkillDir"
        return
    }
    
    foreach ($targetDir in $targetDirs) {
        if (Test-Path $targetDir) {
            try {
                # 复制所有技能文件
                Copy-Item -Path "$sourceSkillDir\*" -Destination $targetDir -Recurse -Force -ErrorAction Stop
                Write-Success "技能文件已复制到: $targetDir"
            } catch {
                Write-Error "复制技能文件失败: $_"
            }
        }
    }
}

# 配置 Claude Code MCP
function Configure-ClaudeMCP {
    Write-Step "配置 Claude Code MCP 服务器..."
    
    $claudeConfigDir = Join-Path $env:USERPROFILE ".claude"
    $claudeConfigFile = Join-Path $claudeConfigDir "settings.json"
    
    if (-not (Test-Path $claudeConfigDir)) {
        New-Item -ItemType Directory -Path $claudeConfigDir -Force | Out-Null
    }
    
    $mcpConfig = @{
        mcpServers = @{
            luft = @{
                command = "luft"
                args = @("mcp", "serve")
            }
        }
    }
    
    try {
        if (Test-Path $claudeConfigFile) {
            # 读取现有配置
            $existingConfig = Get-Content $claudeConfigFile | ConvertFrom-Json
            
            # 合并 MCP 配置
            if ($existingConfig.mcpServers) {
                $existingConfig.mcpServers | ForEach-Object {
                    if ($_.PSObject.Properties.Name -notcontains "luft") {
                        $_ | Add-Member -NotePropertyName "luft" -NotePropertyValue $mcpConfig.mcpServers.luft -Force
                    }
                }
            } else {
                $existingConfig | Add-Member -NotePropertyName "mcpServers" -NotePropertyValue $mcpConfig.mcpServers -Force
            }
            
            # 保存更新后的配置
            $existingConfig | ConvertTo-Json -Depth 10 | Set-Content $claudeConfigFile
        } else {
            # 创建新配置文件
            $mcpConfig | ConvertTo-Json -Depth 10 | Set-Content $claudeConfigFile
        }
        
        Write-Success "Claude Code MCP 配置已更新: $claudeConfigFile"
    } catch {
        Write-Error "配置 Claude Code MCP 失败: $_"
    }
}

# 验证安装
function Test-Installation {
    param([hashtable]$AvailableBackends)
    
    Write-Step "验证安装..."
    
    # 测试 luft 命令
    try {
        $luftVersion = luft --version 2>&1
        if ($LASTEXITCODE -eq 0) {
            Write-Success "Luft CLI 可用"
            if ($Verbose) {
                Write-Host "版本: $luftVersion" -ForegroundColor Gray
            }
        } else {
            Write-Warning "Luft CLI 可能未正确安装"
        }
    } catch {
        Write-Error "无法运行 luft 命令"
    }
    
    # 测试 MCP 服务器
    try {
        $mcpTest = luft mcp-structured-output 2>&1
        if ($LASTEXITCODE -eq 0) {
            Write-Success "MCP 服务器功能可用"
        }
    } catch {
        Write-Warning "MCP 服务器测试失败"
    }
    
    # 测试后端连接
    foreach ($backend in $AvailableBackends.Keys) {
        if ($AvailableBackends[$backend]) {
            try {
                $backendInfo = luft backend check $backend 2>&1
                if ($LASTEXITCODE -eq 0) {
                    Write-Success "后端 '$backend' 可用"
                } else {
                    Write-Warning "后端 '$backend' 检查失败"
                }
            } catch {
                Write-Warning "无法检查后端 '$backend'"
            }
        }
    }
}

# 主安装流程
function Invoke-LuftInstallation {
    Write-Host "🚀 Luft 一键安装脚本" -ForegroundColor Magenta
    Write-Host "================================" -ForegroundColor Cyan
    
    # 1. 检测可用后端
    Write-Step "检测可用的 ACP 后端..."
    $knownBackends = @("mock", "codex", "opencode", "claude")
    $availableBackends = @{}
    
    foreach ($backend in $knownBackends) {
        $isAvailable = Test-BackendAvailable -BackendId $backend
        $availableBackends[$backend] = $isAvailable
        
        if ($isAvailable) {
            Write-Success "后端 '$backend' 可用"
        } else {
            Write-Warning "后端 '$backend' 不可用"
        }
    }
    
    # 2. 安装 Codex ACP（如果需要）
    if (-not $SkipCodexCheck -and -not $availableBackends["codex"]) {
        Write-Step "尝试安装 Codex ACP..."
        if (Install-CodexACP) {
            $availableBackends["codex"] = $true
        }
    }
    
    # 3. 初始化技能目录
    $skillDirs = Initialize-SkillDirectories -AvailableBackends $availableBackends
    
    # 4. 复制技能文件
    if ($skillDirs.Count -gt 0) {
        Copy-SkillFiles -TargetDirs $skillDirs
    }
    
    # 5. 配置 MCP 服务器
    if ($availableBackends["claude"]) {
        Configure-ClaudeMCP
    } else {
        Write-Warning "Claude Code 不可用，跳过 MCP 配置"
        Write-Host "如需使用 MCP，请安装 Claude Code 并重新运行此脚本" -ForegroundColor Gray
    }
    
    # 6. 验证安装
    Test-Installation -AvailableBackends $availableBackends
    
    Write-Host "================================" -ForegroundColor Cyan
    Write-Host "🎉 安装完成！" -ForegroundColor Green
    Write-Host ""
    Write-Host "可用后端:" -ForegroundColor Cyan
    foreach ($backend in $availableBackends.Keys) {
        $status = if ($availableBackends[$backend]) { "✅" } else { "❌" }
        Write-Host "  $status $backend" -ForegroundColor $(if ($availableBackends[$backend]) { "Green" } else { "Red" })
    }
    
    Write-Host ""
    Write-Host "后续步骤:" -ForegroundColor Cyan
    Write-Host "1. 运行 'luft backend list' 查看所有后端" -ForegroundColor White
    Write-Host "2. 运行 'luft backend info' 查看详细后端信息" -ForegroundColor White  
    Write-Host "3. 运行 'luft mcp serve' 启动 MCP 服务器" -ForegroundColor White
}

# 执行安装
try {
    Invoke-LuftInstallation
} catch {
    Write-Error "安装过程中发生错误: $_"
    exit 1
}