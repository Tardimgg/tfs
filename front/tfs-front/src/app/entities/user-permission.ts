import {PermissionType} from './permission-type';

export class UserPermission {
  login: string;
  permission_type: PermissionType;

  constructor(login: string, permission: PermissionType) {
    this.login = login;
    this.permission_type = permission;
  }
}


export let anonymous: string = "anonymous"
