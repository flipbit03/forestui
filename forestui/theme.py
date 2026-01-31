"""Theme and styling for forestui."""

# CSS styles for Textual
APP_CSS = """
$accent: #52B788;
$accent-dark: #2D6A4F;
$bg: #1C1C1E;
$bg-elevated: #2C2C2E;
$bg-hover: #3A3A3C;
$bg-selected: #48484A;
$border: #3D3D3F;
$text-primary: #F5F5F5;
$text-secondary: #A8A8A8;
$text-muted: #7A7A7A;
$destructive: #FF6B6B;
$success: #52B788;
$warning: #FFB347;

Screen {
    background: $bg;
}

/* Main Layout */
#main-container {
    layout: horizontal;
    height: 100%;
}

#sidebar {
    width: 35;
    min-width: 30;
    max-width: 45;
    background: $bg;
    border-right: solid $border;
}

#detail-pane {
    width: 1fr;
    height: 100%;
    background: $bg;
    padding: 1 2;
}

/* Sidebar Header */
.sidebar-header {
    height: 3;
    padding: 1;
    background: $bg-elevated;
    border-bottom: solid $border;
}

.sidebar-header Label {
    text-style: bold;
    color: $text-primary;
}

.sidebar-header .header-buttons {
    dock: right;
}

/* Tree Items */
Tree {
    background: $bg;
    padding: 0 1;
}

Tree > .tree--cursor {
    background: $accent-dark;
    color: $text-primary;
}

Tree > .tree--highlight {
    background: $bg-hover;
}

Tree > .tree--highlight-line {
    background: $bg-hover;
}

/* Buttons */
Button {
    background: $bg-elevated;
    color: $text-primary;
    border: solid $border;
    min-width: 10;
    height: 3;
}

Button:hover {
    background: $bg-hover;
    border: solid $accent;
}

Button:focus {
    border: solid $accent;
}

Button.-primary {
    background: $accent-dark;
    border: solid $accent;
}

Button.-primary:hover {
    background: $accent;
}

Button.-destructive {
    background: #3d2020;
    color: $destructive;
    border: solid #5a3030;
}

Button.-destructive:hover {
    background: #4d2828;
    border: solid $destructive;
}

/* Action Cards */
.action-card {
    background: $bg-elevated;
    border: solid $border;
    padding: 1;
    margin: 0 0 1 0;
    height: auto;
}

.action-card:hover {
    background: $bg-hover;
    border: solid $accent;
}

.action-card:focus {
    border: solid $accent;
}

/* Section Headers */
.section-header {
    text-style: bold;
    color: $text-secondary;
    margin: 1 0 0 0;
    padding: 0 0 0 0;
}

/* Labels */
.label-primary {
    color: $text-primary;
}

.label-secondary {
    color: $text-secondary;
}

.label-muted {
    color: $text-muted;
}

.label-accent {
    color: $accent;
}

.label-destructive {
    color: $destructive;
}

/* Path Display */
.path-display {
    background: $bg-elevated;
    padding: 0 1;
    border: solid $border;
    color: $text-secondary;
}

/* Detail View */
.detail-content {
    height: auto;
    width: 100%;
}

RepositoryDetail {
    height: auto;
    width: 100%;
}

WorktreeDetail {
    height: auto;
    width: 100%;
}

EmptyState {
    height: auto;
    width: 100%;
}

.detail-header {
    height: auto;
    margin-bottom: 1;
}

.detail-title {
    text-style: bold;
    color: $text-primary;
}

.detail-subtitle {
    color: $text-secondary;
}

/* Input Fields */
Input {
    background: $bg-elevated;
    color: $text-primary;
    border: solid $border;
}

Input:focus {
    border: solid $accent;
}

Input.-invalid {
    border: solid $destructive;
}

/* Select */
Select {
    background: $bg-elevated;
    color: $text-primary;
    border: solid $border;
}

Select:focus {
    border: solid $accent;
}

SelectCurrent {
    background: $bg-elevated;
}

SelectOverlay {
    background: $bg-elevated;
    border: solid $border;
}

/* Modals */
ModalScreen {
    align: center middle;
}

.modal-container {
    width: 60;
    max-width: 80%;
    height: auto;
    max-height: 90%;
    background: $bg-elevated;
    border: solid $border;
    padding: 1 2;
}

.modal-scroll {
    height: auto;
    max-height: 20;
}

.modal-title {
    text-style: bold;
    color: $text-primary;
    text-align: center;
    margin-bottom: 1;
}

.modal-buttons {
    margin-top: 1;
    align: center middle;
    height: 3;
}

.modal-buttons Button {
    margin: 0 1;
}

/* Session List */
.session-item {
    background: $bg-elevated;
    border: solid $border;
    padding: 1;
    margin: 0 2 1 0;
    height: auto;
    align: left middle;
}

.session-item:hover {
    background: $bg-hover;
}

.session-header-row {
    width: 100%;
    height: auto;
    align: left middle;
}

.session-info {
    width: 1fr;
    height: auto;
}

.session-title {
    color: $text-primary;
}

.session-last {
    color: $text-secondary;
}

.session-meta {
    color: $text-muted;
    margin-top: 1;
}

.session-buttons {
    height: auto;
    align: right middle;
}

.session-btn {
    min-width: 8;
    height: 3;
    margin-left: 1;
}

/* Status Messages */
.status-bar {
    dock: bottom;
    height: 1;
    background: $bg-elevated;
    color: $text-secondary;
    padding: 0 1;
}

/* Empty State */
.empty-state {
    align: center middle;
    height: 100%;
}

.empty-state Label {
    color: $text-muted;
    text-align: center;
}

/* Collapsible */
Collapsible {
    background: $bg;
    border: none;
    padding: 0;
}

CollapsibleTitle {
    background: $bg;
    color: $text-secondary;
    padding: 0 1;
}

CollapsibleTitle:hover {
    background: $bg-hover;
}

CollapsibleTitle:focus {
    background: $bg-hover;
}

/* OptionList for sidebar */
OptionList {
    background: $bg;
    border: none;
    padding: 0;
}

OptionList > .option-list--option {
    padding: 0 1;
}

OptionList > .option-list--option-highlighted {
    background: $bg-hover;
}

OptionList > .option-list--option-hover {
    background: $bg-hover;
}

/* DataTable */
DataTable {
    background: $bg;
}

DataTable > .datatable--header {
    background: $bg-elevated;
    color: $text-secondary;
    text-style: bold;
}

DataTable > .datatable--cursor {
    background: $accent-dark;
}

/* Horizontal Rule */
Rule.-horizontal {
    color: $border;
    margin: 1 2 1 0;
}

/* Footer */
Footer {
    background: $bg-elevated;
}

FooterKey {
    background: $bg-elevated;
    color: $text-secondary;
}

FooterKey > .footer-key--key {
    background: $accent-dark;
    color: $text-primary;
}

/* ListItem styling */
ListItem {
    background: $bg;
    padding: 0 1;
    height: 1;
}

ListItem:hover {
    background: $bg-hover;
}

ListItem.-selected {
    background: $accent-dark;
}

ListView {
    background: $bg;
}

/* Static content */
Static {
    color: $text-primary;
}

/* Checkbox and RadioButton */
Checkbox {
    background: transparent;
}

Checkbox > .toggle--button {
    background: $bg-elevated;
}

Checkbox:focus > .toggle--button {
    background: $accent-dark;
}

RadioButton {
    background: transparent;
}

RadioSet {
    background: transparent;
    border: none;
}

/* Tabs */
Tabs {
    background: $bg;
}

Tab {
    background: $bg;
    color: $text-secondary;
}

Tab:hover {
    background: $bg-hover;
}

Tab.-active {
    background: $bg-elevated;
    color: $accent;
}

/* ContentSwitcher */
ContentSwitcher {
    background: $bg;
}

/* Containers */
Horizontal {
    height: auto;
}

Vertical {
    height: auto;
}

Container {
    height: auto;
}

/* Scrollbars */
.scrollbar {
    background: $bg-elevated;
}

/* Action row */
.action-row {
    layout: horizontal;
    height: 3;
    margin: 1 0;
}

.action-row Button {
    margin-right: 1;
}

/* Branch tag */
.branch-tag {
    background: $accent-dark;
    color: $accent;
    padding: 0 1;
}

/* Keyboard shortcut badge */
.shortcut-badge {
    background: $bg-elevated;
    color: $text-muted;
    padding: 0 1;
}

/* Sidebar Header Box */
#sidebar-header-box {
    width: 100%;
    height: 3;
    background: $bg-elevated;
    border-bottom: solid $border;
    align: center middle;
    padding: 1 0 0 0;
}

#gh-status {
    text-align: center;
    width: 100%;
    color: $text-muted;
}

.gh-status-ok {
    color: $success;
}

.gh-status-warn {
    color: $warning;
}

.gh-status-error {
    color: $text-muted;
}

/* Async-loaded sections */
#issues-container {
    margin-top: 1;
}

#sessions-container {
    margin-top: 1;
}

/* Section header with refresh button */
.section-header-row {
    height: auto;
    width: auto;
}

.section-header-row .section-header {
    width: auto;
    margin: 0;
}

.refresh-btn {
    min-width: 3;
    width: 3;
    height: 1;
    padding: 0;
    margin: 0;
    border: none;
    background: transparent;
    color: $text-muted;
    text-style: none;
}

.refresh-btn:focus {
    color: $text-muted;
    background: transparent;
    border: none;
    text-style: none;
}

.refresh-btn:hover {
    color: $accent;
    background: transparent;
    border: none;
}

.refresh-btn:focus:hover {
    color: $accent;
    background: transparent;
    border: none;
}

/* Issue List */
.issue-row {
    height: auto;
    margin: 0 2 1 0;
    padding: 1;
    background: $bg-elevated;
    border: solid $border;
}

.issue-row:hover {
    background: $bg-hover;
}

.issue-info {
    width: 1fr;
}

.issue-title {
    color: $text-primary;
}

.issue-meta {
    color: $text-muted;
}

.issue-btn {
    min-width: 12;
    margin-left: 1;
}

.issue-title-preview {
    margin-bottom: 1;
}
"""
