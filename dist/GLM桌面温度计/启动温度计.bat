@echo off
chcp 65001 >nul
echo ============================================
echo   GLM 桌面温度计 v1.2.0
echo ============================================
echo.
echo  即将启动温度计悬浮条（右上角）。
echo  首次运行如被杀软拦截，请选择"允许"并添加信任。
echo.
echo  使用说明：
echo    - 默认鼠标穿透，不挡任何操作
echo    - 按住 Ctrl 悬停：解锁编辑态
echo    - Ctrl + 左键拖拽：移动位置
echo    - Ctrl + 右键：设置菜单（形态/外观/内容/自启）
echo    - Ctrl + 滚轮：胶囊翻页
echo.
start "" "%~dp0tempmon.exe"
echo  已启动。
timeout /t 2 >nul
