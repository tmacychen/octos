#!/usr/bin/env python3
"""
Octos Serve 测试运行脚本

用法:
    python run_serve_tests.py              # 运行所有测试
    python run_serve_tests.py 8.1          # 运行单个测试
    python run_serve_tests.py 8.1 8.2      # 运行多个测试
    python run_serve_tests.py --help       # 显示帮助信息
"""

import argparse
import platform
import subprocess
import sys
from pathlib import Path


def show_help():
    """显示帮助信息"""
    help_text = """
Octos Serve 测试运行脚本

用法:
    python run_serve_tests.py [选项] [测试编号...]

选项:
    --help, -h          显示此帮助信息
    --binary PATH       指定 octos 二进制文件路径
    --verbose, -v       详细输出模式
    --list, -l          列出所有可用测试
    --report            仅显示最新测试报告

测试编号示例:
    python run_serve_tests.py                  # 运行所有测试
    python run_serve_tests.py 8.1              # 运行测试 8.1
    python run_serve_tests.py 8.1 8.2 8.3      # 运行测试 8.1, 8.2, 8.3
    python run_serve_tests.py --verbose        # 详细模式运行所有测试

可用测试:
    8.1  启动服务
    8.2  REST API (/api/sessions)
    8.3  SSE 流式响应
    8.4  Dashboard Web UI
    8.5  Auth Token 认证
    8.6  绑定地址 (--host 0.0.0.0) ⚠️
    8.7  默认绑定本地 (127.0.0.1) ⚠️

⚠️  标记的测试存在环境限制，详见 README.md
"""
    print(help_text)


def list_tests():
    """列出所有可用测试"""
    print("可用的 Serve 测试用例:\n")
    print("  8.1  启动服务 - 验证服务能否正常启动并监听端口")
    print("  8.2  REST API - 验证 /api/sessions 返回 JSON")
    print("  8.3  SSE 流式 - 验证 POST /api/chat 返回流式事件")
    print("  8.4  Dashboard - 验证 Web UI 可以加载")
    print("  8.5  Auth Token - 验证无 token 请求返回 401")
    print("  8.6  绑定地址 - 验证 --host 0.0.0.0 外部可访问 ⚠️")
    print("  8.7  默认绑定 - 验证不加 --host 时默认绑定 127.0.0.1 ⚠️")
    print("\n⚠️  标记的测试存在环境限制，详见 README.md")


def show_latest_report(script_dir: Path):
    """显示最新测试报告"""
    report_dir = script_dir.parent / "test-results"
    
    if not report_dir.exists():
        print("❌ 错误: 测试报告目录不存在")
        sys.exit(1)
    
    reports = sorted(report_dir.glob("SERVE_TEST_REPORT_*.md"), reverse=True)
    
    if not reports:
        print("⚠️  未找到测试报告，请先运行测试")
        sys.exit(1)
    
    latest_report = reports[0]
    print(f"📄 最新测试报告: {latest_report}\n")
    
    with open(latest_report, 'r', encoding='utf-8') as f:
        content = f.read()
        print(content)


def find_binary(binary_path: str = None) -> Path:
    """查找 octos 二进制文件"""
    if binary_path:
        path = Path(binary_path)
        if not path.exists():
            print(f"❌ 错误: 二进制文件不存在: {path}")
            sys.exit(1)
        return path
    
    # 尝试多个位置
    script_dir = Path(__file__).parent
    project_root = script_dir.parent.parent
    
    possible_paths = [
        project_root / "target" / "release" / "octos",
        project_root / "target" / "debug" / "octos",
        Path.home() / ".local" / "bin" / "octos",
        Path("/usr/local/bin") / "octos",
    ]
    
    # Windows 特殊处理
    if platform.system() == "Windows":
        possible_paths = [p.with_suffix(".exe") for p in possible_paths]
    
    for path in possible_paths:
        if path.exists():
            return path
    
    print("❌ 错误: 未找到 octos 二进制文件")
    print("\n请先编译项目:")
    print("  cargo build --release")
    print("\n或指定二进制路径:")
    print(f"  python {sys.argv[0]} --binary /path/to/octos")
    sys.exit(1)


def main():
    parser = argparse.ArgumentParser(
        description="Octos Serve 测试运行脚本",
        add_help=False  # 禁用默认 help，使用自定义
    )
    parser.add_argument('--help', '-h', action='store_true', help='显示帮助信息')
    parser.add_argument('--binary', type=str, default=None, help='octos 二进制文件路径')
    parser.add_argument('--verbose', '-v', action='store_true', help='详细输出模式')
    parser.add_argument('--list', '-l', action='store_true', help='列出所有可用测试')
    parser.add_argument('--report', action='store_true', help='仅显示最新测试报告')
    parser.add_argument('test_ids', nargs='*', help='测试编号列表（如 8.1 8.2）')
    
    args = parser.parse_args()
    
    script_dir = Path(__file__).parent
    
    # 处理特殊命令
    if args.help:
        show_help()
        sys.exit(0)
    
    if args.list:
        list_tests()
        sys.exit(0)
    
    if args.report:
        show_latest_report(script_dir)
        sys.exit(0)
    
    # 查找二进制文件
    binary_path = find_binary(args.binary)
    
    # 检查测试文件
    test_file = script_dir / "test_serve.py"
    if not test_file.exists():
        print(f"❌ 错误: 测试文件不存在: {test_file}")
        sys.exit(1)
    
    # 打印标题
    print("=" * 60)
    print("  Octos Serve 功能测试")
    print("=" * 60)
    print()
    print(f"二进制文件: {binary_path}")
    print(f"测试文件:   {test_file}")
    print()
    
    # 构建 pytest 命令
    pytest_cmd = [sys.executable, "-m", "pytest"]
    
    if not args.test_ids:
        print("🔄 运行所有测试...")
        pytest_cmd.append(str(test_file))
    else:
        print(f"🔄 运行指定测试: {' '.join(args.test_ids)}")
        # 转换测试 ID 为 pytest 节点ID
        for test_id in args.test_ids:
            # 将 8.1 转换为 test_8_1
            test_name = f"test_{test_id.replace('.', '_')}"
            pytest_cmd.append(f"{test_file}::{test_name}")
    
    if args.verbose:
        pytest_cmd.extend(["-v", "-s"])
    
    print()
    print(f"执行命令: {' '.join(pytest_cmd)}")
    print()
    
    # 运行测试
    try:
        result = subprocess.run(pytest_cmd, cwd=script_dir)
        exit_code = result.returncode
    except KeyboardInterrupt:
        print("\n\n⚠️  测试被用户中断")
        sys.exit(130)
    except Exception as e:
        print(f"\n❌ 执行测试失败: {e}")
        sys.exit(1)
    
    print()
    if exit_code == 0:
        print("✅ 所有测试通过!")
    else:
        print("❌ 部分测试失败")
    
    # 显示报告位置
    report_dir = script_dir.parent / "test-results"
    if report_dir.exists():
        reports = sorted(report_dir.glob("SERVE_TEST_REPORT_*.md"), reverse=True)
        if reports:
            latest_report = reports[0]
            print()
            print(f"📄 测试报告: {latest_report}")
            print(f"查看报告: cat \"{latest_report}\"")
    
    sys.exit(exit_code)


if __name__ == "__main__":
    main()
