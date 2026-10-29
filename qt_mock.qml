import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Effects

ApplicationWindow {
    id: app
    title: "Raesir — Qt Quick mock"
    width: 1440; height: 960
    minimumWidth: 1180; minimumHeight: 600
    visible: true
    visibility: Window.Maximized
    flags: Qt.Window | Qt.FramelessWindowHint
    color: "#101916"
    font.family: "Sans Serif"
    font.pixelSize: rem
    property real rem: 16
    property string storeFilter: ""
    property string statusFilter: "all"
    property string osFilter: "all"
    property color colorA: "#203d35"
    property color colorB: "#142b32"
    property string notice: ""
    property bool syncing: false
    Behavior on colorA { ColorAnimation { duration: 450 } }
    Behavior on colorB { ColorAnimation { duration: 450 } }

    function refresh() {
        library.filter(search.text, storeFilter, statusFilter, osFilter)
        grid.positionViewAtBeginning()
    }
    function notify(text) { notice = text; noticeTimer.restart() }
    function reset() {
        search.clear(); storeFilter = ""; statusFilter = "all"; osFilter = "all"; refresh()
    }
    onStoreFilterChanged: refresh()
    onStatusFilterChanged: refresh()
    onOsFilterChanged: refresh()
    Shortcut { sequence: "Ctrl+K"; onActivated: search.forceActiveFocus() }
    Shortcut { sequence: "Ctrl+F"; onActivated: search.forceActiveFocus() }
    Shortcut { sequence: "Ctrl++"; onActivated: app.rem = Math.min(22, app.rem + 1) }
    Shortcut { sequence: "Ctrl+-"; onActivated: app.rem = Math.max(12, app.rem - 1) }
    Shortcut { sequence: "Ctrl+0"; onActivated: app.rem = 16 }

    component Glyph: Image {
        property string name: "all"
        source: "image://mock/icon/" + name
        sourceSize.width: 64; sourceSize.height: 64
        width: 1.2 * app.rem; height: width
        fillMode: Image.PreserveAspectFit
    }
    component IconButton: ToolButton {
        property string glyph: "all"
        property string tip: ""
        property bool selected: false
        property bool light: false
        width: 2.2 * app.rem; height: width
        padding: 0
        focusPolicy: Qt.NoFocus
        contentItem: Item {
            Glyph { name: parent.parent.glyph; anchors.centerIn: parent }
        }
        background: Rectangle {
            radius: width / 2
            color: parent.light ? "#dfede5" : parent.hovered || parent.selected ? "#29d7ebe0" : "transparent"
        }
        ToolTip.visible: hovered && tip.length > 0
        ToolTip.text: tip
        ToolTip.delay: 500
    }
    component GlassPlate: Item {
        id: glass
        function alignCapture() {
            const p = glass.mapToItem(scene, 0, 0)
            sample.sourceRect = Qt.rect(p.x, p.y, glass.width, glass.height)
        }
        Component.onCompleted: Qt.callLater(alignCapture)
        Connections {
            target: app
            function onWidthChanged() { Qt.callLater(glass.alignCapture) }
            function onHeightChanged() { Qt.callLater(glass.alignCapture) }
            function onRemChanged() { Qt.callLater(glass.alignCapture) }
        }
        ShaderEffectSource {
            id: sample
            width: glass.width; height: glass.height
            sourceItem: scene
            textureSize: Qt.size(Math.ceil(width), Math.ceil(height))
            live: true; visible: false
        }
        Rectangle {
            id: mask
            anchors.fill: parent; radius: height / 2
            color: "white"; visible: false; layer.enabled: true
        }
        MultiEffect {
            anchors.fill: parent; source: sample
            blurEnabled: true; blur: 1; blurMax: 16
            autoPaddingEnabled: false
            maskEnabled: true; maskSource: mask
        }
        Rectangle {
            anchors.fill: parent; radius: height / 2
            color: "#b30c1713"
        }
    }
    component Pill: Item {
        default property alias contents: items.data
        width: items.width + .4 * app.rem
        height: 2.6 * app.rem
        GlassPlate { anchors.fill: parent }
        Row { id: items; spacing: .05 * app.rem; anchors.centerIn: parent }
    }

    Item {
        id: scene
        anchors.fill: parent
        Rectangle {
            anchors.fill: parent
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0; color: app.colorA }
                GradientStop { position: 1; color: app.colorB }
            }
            Rectangle {
                anchors.fill: parent
                gradient: Gradient {
                    GradientStop { position: 0; color: "#1007100c" }
                    GradientStop { position: 0.55; color: "#0007100c" }
                    GradientStop { position: 1; color: "#b007100c" }
                }
            }
        }

        GridView {
            id: grid
            objectName: "gameGrid"
            anchors.fill: parent
            anchors.leftMargin: app.rem; anchors.rightMargin: app.rem
            clip: true
            cellWidth: Math.floor(width / Math.max(1, Math.floor(width / (12 * app.rem))))
            cellHeight: Math.ceil((cellWidth - 2 * Math.round(.65 * app.rem)) * 4 / 3 + 3.5 * app.rem)
            pixelAligned: true
            cacheBuffer: height
            reuseItems: true
            boundsBehavior: Flickable.StopAtBounds
            maximumFlickVelocity: 30000
            acceptedButtons: Qt.NoButton
            model: library.games
            header: Item { height: 4.8 * app.rem; width: grid.width }
            footer: Item { height: app.rem; width: grid.width }
            ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }
            delegate: Item {
                id: card
                required property var modelData
                width: grid.cellWidth; height: grid.cellHeight
                property bool local: modelData.installed === "linux"
                HoverHandler {
                    id: cardHover
                    onHoveredChanged: hovered ? hoverTimer.restart() : hoverTimer.stop()
                }
                Timer {
                    id: hoverTimer
                    interval: 600
                    onTriggered: {
                        if (!grid.moving) {
                            app.colorA = card.modelData.colors[0]
                            app.colorB = card.modelData.colors[1]
                        }
                    }
                }
                GridView.onPooled: hoverTimer.stop()
                Item {
                    id: art
                    x: Math.round(.65 * app.rem)
                    width: card.width - 2 * x
                    height: Math.round(width * 4 / 3)
                    Image {
                        anchors.fill: parent
                        source: "image://mock/poster/" + card.modelData.id
                        asynchronous: true
                        mipmap: true
                        smooth: true
                        fillMode: Image.PreserveAspectFit
                    }
                    Rectangle {
                        x: .55 * app.rem; y: x
                        width: 2 * app.rem; height: width; radius: width / 2
                        color: "#df0d1915"
                        Glyph { name: card.modelData.store; anchors.centerIn: parent }
                    }
                    Glyph {
                        anchors.centerIn: parent
                        name: "windows"; width: 2.1 * app.rem
                        visible: card.modelData.installed === "windows"
                    }
                    IconButton {
                        anchors.right: parent.right; anchors.top: parent.top
                        anchors.margins: .5 * app.rem
                        glyph: "more"; tip: "Game actions"
                        visible: cardHover.hovered || actions.opened
                        background: Rectangle { radius: width / 2; color: "#df0d1915" }
                        onClicked: actions.open()
                        Menu {
                            id: actions
                            y: parent.height
                            palette.window: "#182a21"; palette.text: "#e1eee7"
                            MenuItem { text: "Launch"; onTriggered: app.notify("Mock launch — " + card.modelData.title) }
                            MenuItem { text: "View in store"; onTriggered: app.notify("Mock store page — " + card.modelData.title) }
                            MenuItem { text: "Uninstall…"; onTriggered: app.notify("Mock uninstall — " + card.modelData.title) }
                        }
                    }
                    Rectangle {
                        visible: card.local
                        width: .3 * app.rem; height: width; radius: width / 2
                        color: "#91e5ba"
                        anchors.left: parent.left; anchors.bottom: parent.bottom
                        anchors.margins: .85 * app.rem
                    }
                    IconButton {
                        anchors.right: parent.right; anchors.bottom: parent.bottom
                        anchors.margins: .55 * app.rem
                        glyph: card.local ? "play" : "external"
                        tip: card.local ? "Launch (mock)" : "Open store (mock)"
                        light: card.local
                        background: Rectangle { radius: width / 2; color: parent.light ? "#dfede5" : "#df0d1915" }
                        onClicked: app.notify((card.local ? "Mock launch — " : "Mock store page — ") + card.modelData.title)
                    }
                }
                Text {
                    x: art.x; y: art.height + .6 * app.rem
                    width: art.width; height: 2.5 * app.rem
                    text: card.modelData.title
                    color: card.local ? "#edf4ef" : "#a9bbb1"
                    font.pixelSize: .94 * app.rem; font.weight: Font.DemiBold
                    wrapMode: Text.Wrap; maximumLineCount: 2; elide: Text.ElideRight
                }
            }
        }

        Rectangle {
            visible: grid.count === 0
            anchors.horizontalCenter: parent.horizontalCenter
            y: 9 * app.rem; width: 24 * app.rem; height: 17 * app.rem
            radius: 2 * app.rem; color: "#ed0d1a14"
            Column {
                anchors.centerIn: parent; spacing: 1.2 * app.rem
                Glyph { anchors.horizontalCenter: parent.horizontalCenter; width: 2 * app.rem }
                Text { text: "No matching games"; color: "#e4eee7"; font.pixelSize: 1.5 * app.rem; anchors.horizontalCenter: parent.horizontalCenter }
                Text { text: "Try a different filter or search."; color: "#a7bbb0"; font.pixelSize: app.rem; anchors.horizontalCenter: parent.horizontalCenter }
                Button {
                    text: "Clear filters"; anchors.horizontalCenter: parent.horizontalCenter
                    onClicked: app.reset()
                    palette.button: "#273e31"; palette.buttonText: "#e4eee7"
                }
            }
        }

    }

    Item {
        id: header
        z: 2; width: parent.width; height: 4.5 * app.rem
        DragHandler { target: null; onActiveChanged: if (active) app.startSystemMove() }
        TapHandler { onDoubleTapped: app.visibility === Window.Maximized ? app.showNormal() : app.showMaximized() }
        Pill {
            anchors.left: parent.left; anchors.leftMargin: app.rem
            anchors.verticalCenter: parent.verticalCenter
            Repeater {
                model: ["all", "steam", "epic", "ea", "ubisoft", "xbox"]
                IconButton {
                    required property string modelData
                    glyph: modelData
                    selected: app.storeFilter === (modelData === "all" ? "" : modelData)
                    tip: modelData === "all" ? "All platforms" : modelData
                    onClicked: app.storeFilter = modelData === "all" ? "" : modelData
                }
            }
        }
        Item {
            anchors.centerIn: parent
            width: Math.min(25 * app.rem, parent.width - platformsRight.width * 2 - 2 * app.rem)
            height: 2.6 * app.rem
            GlassPlate { anchors.fill: parent }
            Glyph { name: "search"; x: .8 * app.rem; anchors.verticalCenter: parent.verticalCenter }
            TextField {
                id: search
                anchors.fill: parent; anchors.leftMargin: 2.3 * app.rem; anchors.rightMargin: .8 * app.rem
                padding: 0; background: null
                placeholderText: "Search games"
                placeholderTextColor: "#80968a"; color: "#e4eee7"; font.pixelSize: app.rem
                selectByMouse: true
                onTextChanged: searchTimer.restart()
            }
        }
        Row {
            id: platformsRight
            anchors.right: parent.right; anchors.rightMargin: app.rem
            anchors.verticalCenter: parent.verticalCenter
            spacing: .6 * app.rem
            Pill {
                Repeater {
                    model: ["all", "installed", "cloud"]
                    IconButton {
                        required property string modelData
                        glyph: modelData; selected: app.statusFilter === modelData
                        tip: modelData === "all" ? "All games" : modelData === "installed" ? "Installed" : "Not installed"
                        onClicked: app.statusFilter = modelData
                    }
                }
            }
            Pill {
                Repeater {
                    model: ["globe", "this", "other"]
                    IconButton {
                        required property string modelData
                        glyph: modelData; selected: app.osFilter === (modelData === "globe" ? "all" : modelData)
                        tip: modelData === "globe" ? "All operating systems" : modelData === "this" ? "This OS" : "Other OS"
                        onClicked: app.osFilter = modelData === "globe" ? "all" : modelData
                    }
                }
            }
            Pill {
                IconButton {
                    glyph: "sync"; tip: "Sync (mock)"; enabled: !app.syncing
                    onClicked: { app.syncing = true; syncTimer.restart() }
                    RotationAnimator on rotation { running: app.syncing; from: 0; to: 360; duration: 800; loops: Animation.Infinite }
                }
                IconButton {
                    glyph: "menu"; tip: "Options"
                    onClicked: options.open()
                    Menu {
                        id: options
                        y: parent.height
                        MenuItem { text: "Qt Quick · 240 mocked games"; enabled: false }
                        MenuItem { text: "Native scrolling · simulated actions"; enabled: false }
                        MenuItem { text: "Quit mock"; onTriggered: app.close() }
                    }
                }
            }
            Pill {
                IconButton { glyph: "minimize"; tip: "Minimize"; onClicked: app.showMinimized() }
                IconButton { glyph: "maximize"; tip: "Maximize / restore"; onClicked: app.visibility === Window.Maximized ? app.showNormal() : app.showMaximized() }
                IconButton { glyph: "close"; tip: "Close"; onClicked: app.close() }
            }
        }
    }
    Rectangle {
        z: 3; visible: app.notice.length > 0
        anchors.horizontalCenter: parent.horizontalCenter; anchors.bottom: parent.bottom; anchors.bottomMargin: 2 * app.rem
        width: message.width + 3 * app.rem; height: 3.2 * app.rem
        radius: height / 2; color: "#f0182a20"
        Text { id: message; anchors.centerIn: parent; text: app.notice; color: "#edf4ef"; font.pixelSize: app.rem }
    }
    Timer { id: searchTimer; interval: 150; onTriggered: app.refresh() }
    Timer { id: noticeTimer; interval: 2200; onTriggered: app.notice = "" }
    Timer { id: syncTimer; interval: 800; onTriggered: { app.syncing = false; app.notify("Mock library refreshed") } }
}
