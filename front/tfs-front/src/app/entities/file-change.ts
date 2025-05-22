export class FileChange {
  changeType: FileChangeType


  constructor(changeType: FileChangeType) {
    this.changeType = changeType;
  }
}

export enum FileChangeType {
  Add,
  Delete,
  Replace
}

export function castFileChangeTypeToE(changeType: FileChangeType) {
  switch (changeType) {
    case FileChangeType.Add:
      return "Добавить";
    case FileChangeType.Delete:
      return "Удалить";
    case FileChangeType.Replace:
      return "Заменить";
  }
}
