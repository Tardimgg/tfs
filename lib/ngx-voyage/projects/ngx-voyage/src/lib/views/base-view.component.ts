import {
  Component,
  computed,
  contentChild,
  inject,
  input,
  model,
  OnChanges,
  output,
  signal,
  SimpleChanges,
  TemplateRef,
  viewChild,
} from "@angular/core";
import { MenuItem } from "primeng/api";
import { ContextMenu } from "primeng/contextmenu";
import { canPreviewFile, getFileIcon } from "../model/file-types";
import {
  File,
  FilePreviewOutput,
  FileSortFields,
  isFileEqual,
  sortFiles,
} from "../model/model";
import { Store } from "../model/store";
@Component({
  template: ``,
})
export abstract class BaseViewComponent implements OnChanges {
  store = inject(Store);

  path = model.required<string>();
  files = input.required<File[]>();

  openFile = output<string>();
  permissionsManagement = output<string>();
  modifyFile = output<string>();
  deleteFile = output<string>();
  previewFile = output<FilePreviewOutput>();

  selectedFile = this.store.selectedFile;
  showPreview = model(false);
  previewData = signal<Blob | undefined>(undefined);

  emptyFiles = contentChild<TemplateRef<Element>>("emptyFiles");

  contextMenu = viewChild<ContextMenu>("contextMenu");
  getFileIcon = getFileIcon;

  filteredFiles = computed(() => {
    if (this.store.showHiddenFiles()) {
      return this.files();
    } else {
      return this.files().filter(({ name }) => !name.startsWith("."));
    }
  });
  sortOrder = signal<number>(0);
  sortField = signal<FileSortFields | undefined>(undefined);
  sortedFiles = computed(() => {
    if (this.sortOrder() == undefined || this.sortField() == undefined) {
      return this.filteredFiles();
    }
    return sortFiles(
      [...this.filteredFiles()],
      this.sortField(),
      this.sortOrder(),
    );
  });

  menuItems: MenuItem[] = [
    {
      label: "Посмотреть",
      visible: false,
      command: () => {
        const f = this.selectedFile();
        if (f) {
          this.openFilePreview(f);
        }
      },
    },
    {
      label: "Открыть",
      command: () => {
        const f = this.selectedFile();
        if (f) {
          this.openFileOrFolder(f);
        }
      },
    },
    {
      label: "Управлять разрешениями",
      command: () => {
        const f = this.selectedFile();
        if (f) {
          this.openPermissionManagement(f);
        }
      },
    },
    {
      label: "Изменить",
      command: () => {
        const f = this.selectedFile();
        if (f) {
          this.openModify(f);
        }
      },
    },
    {
      label: "Удалить",
      command: () => {
        const f = this.selectedFile();
        if (f) {
          this.openDelete(f);
        }
      },
    },
  ];

  ngOnChanges(changes: SimpleChanges) {
    if (changes["path"] && !changes["path"].firstChange) {
      this.selectedFile.set(undefined);
      this.showPreview.set(false);
    }
  }

  onDoubleClick(file: File) {
    if (canPreviewFile(file) && this.store.showPreviewFile()) {
      this.selectedFile.set(file);
      this.openFilePreview(file);
    } else {
      this.openFileOrFolder(file);
    }
  }

  onMouseDown(event: MouseEvent) {
    // when using double click to open a file,
    // prevent the text node of the file name to be selected
    if (event.detail > 1) {
      event.preventDefault();
    }
  }

  openFilePreview(file: File) {
    const path = this.getTargetPath(file);
    this.previewFile.emit({
      path,
      cb: (data) => {
        this.previewData.set(data);
        this.showPreview.set(true);
      },
    });
  }

  openFileOrFolder(file: File) {
    const targetPath = this.getTargetPath(file);
    if (file.isDirectory) {
      this.path.set(targetPath);
    } else if (this.store.showOpenFile()) {
      this.openFile.emit(targetPath);
    }
  }

  openPermissionManagement(file: File) {
    const targetPath = this.getTargetPath(file);
    this.permissionsManagement.emit(targetPath)
  }
  openModify(file: File) {
    const targetPath = this.getTargetPath(file);
    this.modifyFile.emit(targetPath)
  }

  openDelete(file: File) {
    const targetPath = this.getTargetPath(file);
    this.deleteFile.emit(targetPath)
  }

  onContextMenu(event: MouseEvent, file: File) {
    const cm = this.contextMenu();
    if (cm && event?.currentTarget && file) {
      this.selectedFile.set(file);
      this.menuItems[0].visible =
        this.store.showPreviewFile() && canPreviewFile(file);
      this.menuItems[1].visible = this.store.showOpenFile() || file.isDirectory;
      this.menuItems[2].visible = this.store.showPermissionManager();
      this.menuItems[3].visible = file.isFile && this.store.showModifyFile();
      this.menuItems[4].visible = this.store.showDeleteFile();
      if (!this.menuItems[0].visible && !this.menuItems[1].visible &&
        !this.menuItems[2].visible && !this.menuItems[3].visible && this.menuItems[4].visible) {
        return;
      }
      cm.target = event.currentTarget as HTMLElement;
      cm.show(event);
    }
  }

  getTargetPath(file: File) {
    return `${this.path()}/${file.name}`.replaceAll("//", "/");
  }

  isSelectedFile(file: File) {
    return this.selectedFile() === file;
  }

  selectNextOrPreviousFile(offset: -1 | 1) {
    const selected = this.selectedFile();
    if (selected == undefined) {
      this.selectFirstFile();
    } else {
      for (let i = 0; i < this.sortedFiles().length; i++) {
        const file = this.sortedFiles()[i];
        if (
          isFileEqual(file, selected) &&
          i + offset >= 0 &&
          i + offset < this.sortedFiles().length
        ) {
          this.selectFile(this.sortedFiles()[i + offset]);
          break;
        }
      }
    }
  }

  selectFirstFile() {
    this.selectFile(this.sortedFiles()[0]);
  }

  selectFile(file: File) {
    for (let i = 0; i < this.sortedFiles().length; i++) {
      const f = this.sortedFiles()[i];
      if (isFileEqual(file, f)) {
        const fileDom = document.querySelector(
          `[data-fileIndex="${i}"]`,
        ) as HTMLTableRowElement;
        fileDom.focus();
      }
    }
    this.selectedFile.set(file);
  }
}
